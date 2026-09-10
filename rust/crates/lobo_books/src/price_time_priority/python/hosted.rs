//! Optional Python construction scope and shared native book handle.
//! The server implements command routing; this module has no transport dependency.
use super::{Book, CommandResult, PyExecution, PyFill, PyReports};
use lobo_models::{
    events::Reports,
    orders::{
        core::PyOrder,
        order_types::{
            IcebergOrder, LimitOrder, MarketOrder, PyIcebergOrder, PyLimitOrder, PyMarketOrder,
        },
    },
    server::{BookPolicy, Command, Order, OrderFields},
};
use lobo_primitives::{CompressedPrice, PriceType};
use lobo_storage::{
    UserMapUpdatePolicy,
    policies::HiddenQuantityPolicy,
    price_level::PriceLevelContract,
    price_sorting::PriceSortingPolicy,
};
use parking_lot::Mutex;
use pyo3::{
    exceptions::{PyRuntimeError, PyTypeError},
    prelude::*,
    sync::PyOnceLock,
    types::PyDict,
};
use std::sync::Arc;

pub trait HostedCommands: Send + Sync {
    fn apply(
        &self,
        command: Command,
        reports: Reports,
    ) -> Result<CommandResult<CompressedPrice>, String>;
}
pub struct HostedBook<
    L: PriceLevelContract,
    S: PriceSortingPolicy,
    U: UserMapUpdatePolicy,
    H: HiddenQuantityPolicy,
> {
    pub native: Arc<Mutex<Book<L, S, U, H>>>,
    pub commands: Arc<dyn HostedCommands>,
}
pub type Factory = Arc<dyn Fn(Option<String>, BookPolicy, lobo_models::CheckSum) -> PyResult<super::PyBook> + Send + Sync>;
#[pyclass]
struct BookFactory {
    create: Factory,
}
static ACTIVE_FACTORY: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
fn variable(py: Python<'_>) -> PyResult<&Py<PyAny>> {
    ACTIVE_FACTORY.get_or_try_init(py, || {
        let kwargs = PyDict::new(py);
        kwargs.set_item("default", py.None())?;
        Ok(py
            .import("contextvars")?
            .getattr("ContextVar")?
            .call(("lobo_server_context",), Some(&kwargs))?
            .unbind())
    })
}
/// ContextVar tokens preserve nested scopes and isolate async Python tasks.
pub fn enter(py: Python<'_>, factory: Factory) -> PyResult<Py<PyAny>> {
    let factory = Py::new(py, BookFactory { create: factory })?;
    Ok(variable(py)?
        .bind(py)
        .call_method1("set", (factory,))?
        .unbind())
}
pub fn exit(py: Python<'_>, token: Py<PyAny>) -> PyResult<()> {
    variable(py)?.bind(py).call_method1("reset", (token,))?;
    Ok(())
}
pub fn create_book(
    py: Python<'_>,
    name: Option<String>,
    policy: BookPolicy,
    checksum: lobo_models::CheckSum,
) -> PyResult<Option<super::PyBook>> {
    let active = variable(py)?.bind(py).call_method0("get")?;
    if active.is_none() {
        return Ok(None);
    }
    let factory = active.cast::<BookFactory>()?.borrow().create.clone();
    factory(name, policy, checksum).map(Some)
}
pub fn fill(
    commands: Arc<dyn HostedCommands>,
    order: Bound<'_, PyOrder>,
    reports: Option<PyRef<'_, PyReports>>,
    execution: PyExecution,
) -> PyResult<Py<PyFill>> {
    let py = order.py();
    let reports = reports.map_or_else(Reports::default, |reports| reports.reports);
    let spec = if let Ok(value) = order.cast::<PyLimitOrder>() {
        let order: LimitOrder<CompressedPrice> = (&*value.borrow()).into();
        Order::Limit(lobo_models::server::LimitOrder {
            fields: OrderFields::from_core(&order.common_data),
            price: order
                .common_data
                .price
                .ok_or_else(|| PyTypeError::new_err("limit order needs a price"))?
                .into_u128() as u32,
        })
    } else if let Ok(value) = order.cast::<PyMarketOrder>() {
        let order: MarketOrder<CompressedPrice> = (&*value.borrow()).into();
        Order::Market(lobo_models::server::MarketOrder {
            fields: OrderFields::from_core(&order.common_data),
        })
    } else if let Ok(value) = order.cast::<PyIcebergOrder>() {
        let order: IcebergOrder<CompressedPrice> = (&*value.borrow()).into();
        Order::Iceberg(lobo_models::server::IcebergOrder {
            fields: OrderFields::from_core(&order.common_data),
            price: order
                .common_data
                .price
                .ok_or_else(|| PyTypeError::new_err("iceberg order needs a price"))?
                .into_u128() as u32,
            hidden_quantity: order.typed_order_details.hidden_quantity,
            peak_quantity: order.typed_order_details.peak_quantity,
        })
    } else {
        return Err(PyTypeError::new_err("unsupported order type"));
    };
    let command = match execution {
        PyExecution::Mutating => Command::Fill { order: spec },
        PyExecution::Simulation => Command::Simulate { order: spec },
    };
    let result = py
        .detach(|| commands.apply(command, reports))
        .map_err(PyRuntimeError::new_err)?;
    PyFill::from_command_result(py, result)?.into_python(py)
}
