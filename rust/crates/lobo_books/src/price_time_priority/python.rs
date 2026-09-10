use super::{Book, Command, CommandError, CommandResult};
pub mod hosted;
pub mod policies;
use lobo_events::{MpscPublisher, NullPublisher, PriceLevelChangeEvent};
use lobo_models::{
    Side,
    events::{Fill as MatchedFill, MarketImpact, Report, Reports, Summary},
    orders::{
        core::{Order, PyOrder},
        order_types::{
            IcebergOrder, IcebergOrderData, LimitOrder, LimitOrderData, MarketOrder,
            MarketOrderData, PyIcebergOrder, PyLimitOrder, PyMarketOrder,
        },
        traits::{HandlesCompletion, IntoRestingOrderData},
    },
};
use lobo_primitives::{
    CompressedPrice,
    mixins::{DisplayFields, PyDisplay},
    uuid::Uuid,
};
use lobo_storage::{
    MutatingFills, SimulatedFills, UpdateUserMap, UserMapUpdatePolicy, UserOutstandingLiquidity,
    policies::{HiddenQuantityPolicy, UpdateHiddenQuantity},
    price_level::{DeepPriceLevel, PriceLevelContract},
    price_sorting::{PriceSortingPolicy, SortedVectorPriceSorting},
};
use pyo3::{
    exceptions::{PyRuntimeError, PyTypeError},
    prelude::*,
};

fn command_error(error: CommandError) -> PyErr {
    match error {
        CommandError::UserStateError(_) => PyRuntimeError::new_err("user state error"),
        CommandError::OrderStateError(_) => PyRuntimeError::new_err("order state error"),
    }
}

pub fn py_best_bid<L, S, U, H, Pub>(book: &Book<L, S, U, H, Pub>) -> Option<CompressedPrice>
where
    L: PriceLevelContract<Price = CompressedPrice> + Send,
    S: PriceSortingPolicy + Send,
    U: UserMapUpdatePolicy + Send,
    H: HiddenQuantityPolicy + Send,
    Pub: lobo_events::BookPublisherFactory<CompressedPrice> + Send,
    Book<L, S, U, H, Pub>: Send,
{
    let (_, level) = book.order_storage.bids.best()?;
    Some(level.price())
}

pub fn py_best_ask<L, S, U, H, Pub>(book: &Book<L, S, U, H, Pub>) -> Option<CompressedPrice>
where
    L: PriceLevelContract<Price = CompressedPrice> + Send,
    S: PriceSortingPolicy + Send,
    U: UserMapUpdatePolicy + Send,
    H: HiddenQuantityPolicy + Send,
    Pub: lobo_events::BookPublisherFactory<CompressedPrice> + Send,
    Book<L, S, U, H, Pub>: Send,
{
    let (_, level) = book.order_storage.asks.best()?;
    Some(level.price())
}

pub fn py_visible_quantity<L, S, U, H, Pub>(book: &Book<L, S, U, H, Pub>, side: Side) -> u64
where
    L: PriceLevelContract<Price = CompressedPrice> + Send,
    S: PriceSortingPolicy + Send,
    U: UserMapUpdatePolicy + Send,
    H: HiddenQuantityPolicy + Send,
    Pub: lobo_events::BookPublisherFactory<CompressedPrice> + Send,
    Book<L, S, U, H, Pub>: Send,
{
    match side {
        Side::Buy => book.order_storage.bids.visible_quantity,
        Side::Sell => book.order_storage.asks.visible_quantity,
    }
}

pub fn py_hidden_quantity<L, S, U, H, Pub>(book: &Book<L, S, U, H, Pub>, side: Side) -> u64
where
    L: PriceLevelContract<Price = CompressedPrice> + Send,
    S: PriceSortingPolicy + Send,
    U: UserMapUpdatePolicy + Send,
    H: HiddenQuantityPolicy + Send,
    Pub: lobo_events::BookPublisherFactory<CompressedPrice> + Send,
    Book<L, S, U, H, Pub>: Send,
{
    match side {
        Side::Buy => book.order_storage.bids.hidden_quantity,
        Side::Sell => book.order_storage.asks.hidden_quantity,
    }
}

pub fn py_order_count<L, S, U, H, Pub>(book: &Book<L, S, U, H, Pub>, side: Side) -> usize
where
    L: PriceLevelContract<Price = CompressedPrice> + Send,
    S: PriceSortingPolicy + Send,
    U: UserMapUpdatePolicy + Send,
    H: HiddenQuantityPolicy + Send,
    Pub: lobo_events::BookPublisherFactory<CompressedPrice> + Send,
    Book<L, S, U, H, Pub>: Send,
{
    match side {
        Side::Buy => book.order_storage.bids.len(),
        Side::Sell => book.order_storage.asks.len(),
    }
}

#[cfg(feature = "python-polars")]
pub fn py_levels_df<L, S, U, H, Pub>(
    book: &Book<L, S, U, H, Pub>,
    py: Python<'_>,
    side: Side,
    n_rows: Option<usize>,
) -> PyResult<Py<PyAny>>
where
    L: PriceLevelContract<Price = CompressedPrice> + Send,
    S: PriceSortingPolicy + Send,
    U: UserMapUpdatePolicy + Send,
    H: HiddenQuantityPolicy + Send,
    Pub: lobo_events::BookPublisherFactory<CompressedPrice> + Send,
    Book<L, S, U, H, Pub>: Send,
{
    let frame = match side {
        Side::Buy => book.order_storage.bids.price_levels_df(n_rows)?,
        Side::Sell => book.order_storage.asks.price_levels_df(n_rows)?,
    };
    Ok(frame.into_pyobject(py)?.unbind())
}

pub fn py_user_order_summary<L, S, U, H, Pub>(
    book: &mut Book<L, S, U, H, Pub>,
    py: Python<'_>,
    trader: Uuid,
) -> PyResult<Py<UserOutstandingLiquidity>>
where
    L: PriceLevelContract<Price = CompressedPrice> + Send,
    S: PriceSortingPolicy + Send,
    U: UserMapUpdatePolicy + Send,
    H: HiddenQuantityPolicy + Send,
    Pub: lobo_events::BookPublisherFactory<CompressedPrice> + Send,
    Book<L, S, U, H, Pub>: Send,
{
    book.order_storage
        .user_outstanding_liquidity(trader)
        .into_python(py)
}

pub fn py_fill<L, S, U, H, Pub>(
    book: &mut Book<L, S, U, H, Pub>,
    order: Bound<'_, PyOrder>,
    reports: Option<PyRef<'_, PyReports>>,
    execution: PyExecution,
) -> PyResult<Py<PyFill>>
where
    L: PriceLevelContract<Price = CompressedPrice> + Send,
    S: PriceSortingPolicy + Send,
    U: UserMapUpdatePolicy + Send,
    H: HiddenQuantityPolicy + Send,
    Pub: lobo_events::BookPublisherFactory<CompressedPrice> + Send,
    Book<L, S, U, H, Pub>: Send,
{
    let py = order.py();
    let reports = reports.map_or_else(Reports::default, |reports| reports.reports);

    let result = if let Ok(order) = order.cast::<PyLimitOrder>() {
        let order: LimitOrder<CompressedPrice> = (&*order.borrow()).into();
        py.detach(|| {
            submit_python_fill::<L, S, U, H, Pub, LimitOrderData, LimitOrderData>(
                book, order, execution, reports,
            )
        })
        .map_err(command_error)?
    } else if let Ok(order) = order.cast::<PyMarketOrder>() {
        let order: MarketOrder<CompressedPrice> = (&*order.borrow()).into();
        py.detach(|| {
            submit_python_fill::<L, S, U, H, Pub, MarketOrderData, LimitOrderData>(
                book, order, execution, reports,
            )
        })
        .map_err(command_error)?
    } else if let Ok(order) = order.cast::<PyIcebergOrder>() {
        let order: IcebergOrder<CompressedPrice> = (&*order.borrow()).into();
        py.detach(|| {
            submit_python_fill::<L, S, U, H, Pub, IcebergOrderData, IcebergOrderData>(
                book, order, execution, reports,
            )
        })
        .map_err(command_error)?
    } else {
        return Err(PyTypeError::new_err("unsupported order type"));
    };

    PyFill::from_command_result(py, result)?.into_python(py)
}

pub fn py_cancel<L, S, U, H, Pub>(
    book: &mut Book<L, S, U, H, Pub>,
    order_id: Uuid,
) -> PyResult<PyCancel>
where
    L: PriceLevelContract<Price = CompressedPrice> + Send,
    S: PriceSortingPolicy + Send,
    U: UserMapUpdatePolicy + Send,
    H: HiddenQuantityPolicy + Send,
    Pub: lobo_events::BookPublisherFactory<CompressedPrice> + Send,
    Book<L, S, U, H, Pub>: Send,
{
    book.submit::<LimitOrderData, LimitOrderData, MutatingFills>(Command::Cancel { order_id })
        .map_err(command_error)?
        .try_into()
}

fn submit_python_fill<L, S, U, H, Pub, T, R>(
    book: &mut Book<L, S, U, H, Pub>,
    order: Order<T, CompressedPrice>,
    execution: PyExecution,
    reports: Reports,
) -> Result<CommandResult<CompressedPrice>, CommandError>
where
    L: PriceLevelContract<Price = CompressedPrice> + Send,
    S: PriceSortingPolicy + Send,
    U: UserMapUpdatePolicy + Send,
    H: HiddenQuantityPolicy + Send,
    Pub: lobo_events::BookPublisherFactory<CompressedPrice> + Send,
    Book<L, S, U, H, Pub>: Send,
    Order<T, CompressedPrice>: HandlesCompletion<CompressedPrice>,
    R: IntoRestingOrderData,
{
    match execution {
        PyExecution::Mutating => book.submit::<T, R, MutatingFills>(Command::Fill {
            order,
            execution: MutatingFills,
            reports,
        }),
        PyExecution::Simulation => book.submit::<T, R, SimulatedFills>(Command::Fill {
            order,
            execution: SimulatedFills,
            reports,
        }),
    }
}

/// Runtime selection occurs at the Python boundary; each native mutation still
/// uses a concrete publisher, so the default null path retains static dispatch.
pub enum PythonBook<
    L: PriceLevelContract,
    S: PriceSortingPolicy,
    U: UserMapUpdatePolicy,
    H: HiddenQuantityPolicy,
> {
    Null(Book<L, S, U, H>),
    Publishing(Book<L, S, U, H, MpscPublisher<PriceLevelChangeEvent<L::Price>>>),
    Hosted(hosted::HostedBook<L, S, U, H>),
}

impl<L: PriceLevelContract, S: PriceSortingPolicy, U: UserMapUpdatePolicy, H: HiddenQuantityPolicy>
    PythonBook<L, S, U, H>
{
    /// Disconnect at context exit while preserving books held by Python callers.
    pub fn disconnect(&mut self) {
        if matches!(self, Self::Null(_) | Self::Hosted(_)) {
            return;
        }
        let previous = std::mem::replace(self, Self::Null(Book::new()));
        if let Self::Publishing(book) = previous {
            *self = Self::Null(book.with_publisher(NullPublisher));
        }
    }
}

#[macro_export]
macro_rules! dispatch_python_book {
    ($value:expr, $book:ident, $call:expr) => {
        match $value {
            $crate::price_time_priority::python::PythonBook::Null($book) => $call,
            $crate::price_time_priority::python::PythonBook::Publishing($book) => $call,
            $crate::price_time_priority::python::PythonBook::Hosted(hosted) => {
                let mut guard = hosted.native.lock();
                let $book = &mut *guard;
                $call
            }
        }
    };
}

mod book;
pub use book::{BookAccess, PyBook, PyStorage, PyBidAskStoreView};
pub type NativeBook = Book<DeepPriceLevel<CompressedPrice, UpdateHiddenQuantity>, SortedVectorPriceSorting, UpdateUserMap, UpdateHiddenQuantity>;

#[pyclass(name = "Execution", module = "lobo", eq, eq_int, from_py_object)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PyExecution {
    #[default]
    Mutating,
    Simulation,
}

#[pyclass(name = "Reports", module = "lobo", extends = PyDisplay)]
pub struct PyReports {
    reports: Reports,
}

#[pymethods]
impl PyReports {
    #[new]
    #[pyo3(signature = (include_fills=false, include_summary=false, include_market_impact=false))]
    fn new(
        include_fills: bool,
        include_summary: bool,
        include_market_impact: bool,
    ) -> PyClassInitializer<Self> {
        PyClassInitializer::from(PyDisplay).add_subclass(Self {
            reports: Reports {
                include_fills,
                include_summary,
                include_market_impact,
            },
        })
    }

    #[getter]
    fn include_fills(&self) -> bool {
        self.reports.include_fills
    }

    #[getter]
    fn include_summary(&self) -> bool {
        self.reports.include_summary
    }

    #[getter]
    fn include_market_impact(&self) -> bool {
        self.reports.include_market_impact
    }

    #[classattr]
    fn __display_fields__() -> Vec<&'static str> {
        Self::display_fields()
    }
}

impl DisplayFields for PyReports {
    fn display_fields() -> Vec<&'static str> {
        vec!["include_fills", "include_summary", "include_market_impact"]
    }
}

#[pyclass(name = "Summary", module = "lobo", get_all, extends = PyDisplay)]
pub struct PySummary {
    pub order_id: Uuid,
    pub trader_id: Uuid,
    pub side: Side,
    pub filled_quantity: u64,
    pub realized_price: CompressedPrice,
}

#[pymethods]
impl PySummary {
    #[classattr]
    fn __display_fields__() -> Vec<&'static str> {
        Self::display_fields()
    }
}

impl DisplayFields for PySummary {
    fn display_fields() -> Vec<&'static str> {
        vec![
            "order_id",
            "trader_id",
            "side",
            "filled_quantity",
            "realized_price",
        ]
    }
}

impl From<Summary<CompressedPrice>> for PySummary {
    fn from(summary: Summary<CompressedPrice>) -> Self {
        Self {
            order_id: summary.order_id,
            trader_id: summary.trader_id,
            side: summary.side,
            filled_quantity: summary.filled_quantity,
            realized_price: summary.realized_price,
        }
    }
}

impl PySummary {
    fn into_python(self, py: Python<'_>) -> PyResult<Py<Self>> {
        Py::new(py, PyClassInitializer::from(PyDisplay).add_subclass(self))
    }
}

#[pyclass(name = "MarketImpact", module = "lobo", get_all, extends = PyDisplay)]
pub struct PyMarketImpact {
    pub avg_price: f64,
    pub worst_price: CompressedPrice,
    pub slippage: CompressedPrice,
    pub slippage_bps: f64,
    pub levels_consumed: usize,
    pub total_quantity_available: u64,
}

#[pymethods]
impl PyMarketImpact {
    fn can_fill(&self, requested_quantity: u64) -> bool {
        self.total_quantity_available >= requested_quantity
    }

    fn fill_ratio(&self, requested_quantity: u64) -> f64 {
        if requested_quantity == 0 {
            1.0
        } else {
            (self.total_quantity_available as f64 / requested_quantity as f64).min(1.0)
        }
    }

    #[classattr]
    fn __display_fields__() -> Vec<&'static str> {
        Self::display_fields()
    }
}

impl DisplayFields for PyMarketImpact {
    fn display_fields() -> Vec<&'static str> {
        vec![
            "avg_price",
            "worst_price",
            "slippage",
            "slippage_bps",
            "levels_consumed",
            "total_quantity_available",
        ]
    }
}

impl From<MarketImpact<CompressedPrice>> for PyMarketImpact {
    fn from(market_impact: MarketImpact<CompressedPrice>) -> Self {
        Self {
            avg_price: market_impact.avg_price,
            worst_price: market_impact.worst_price,
            slippage: market_impact.slippage,
            slippage_bps: market_impact.slippage_bps,
            levels_consumed: market_impact.levels_consumed,
            total_quantity_available: market_impact.total_quantity_available,
        }
    }
}

impl PyMarketImpact {
    fn into_python(self, py: Python<'_>) -> PyResult<Py<Self>> {
        Py::new(py, PyClassInitializer::from(PyDisplay).add_subclass(self))
    }
}

#[pyclass(name = "MatchedFill", module = "lobo", get_all, extends = PyDisplay)]
pub struct PyMatchedFill {
    pub maker_order_id: Uuid,
    pub taker_order_id: Uuid,
    pub fill_quantity: u64,
    pub maker_depleted: bool,
    pub maker_trader_id: Uuid,
    pub price: CompressedPrice,
    pub fill_time: String,
}

#[pymethods]
impl PyMatchedFill {
    #[classattr]
    fn __display_fields__() -> Vec<&'static str> {
        Self::display_fields()
    }
}

impl DisplayFields for PyMatchedFill {
    fn display_fields() -> Vec<&'static str> {
        vec![
            "maker_order_id",
            "taker_order_id",
            "fill_quantity",
            "maker_depleted",
            "maker_trader_id",
            "price",
            "fill_time",
        ]
    }
}

impl From<MatchedFill<CompressedPrice>> for PyMatchedFill {
    fn from(fill: MatchedFill<CompressedPrice>) -> Self {
        Self {
            maker_order_id: fill.maker_order_id,
            taker_order_id: fill.taker_order_id,
            fill_quantity: fill.fill_quantity,
            maker_depleted: fill.maker_depleted,
            maker_trader_id: fill.maker_trader_uid,
            price: fill.price,
            fill_time: fill.fill_time.to_rfc3339(),
        }
    }
}

impl PyMatchedFill {
    fn into_python(self, py: Python<'_>) -> PyResult<Py<Self>> {
        Py::new(py, PyClassInitializer::from(PyDisplay).add_subclass(self))
    }
}

#[pyclass(name = "Report", module = "lobo", extends = PyDisplay)]
pub struct PyReport {
    fills: Option<Vec<Py<PyMatchedFill>>>,
    summary: Option<Py<PySummary>>,
    market_impact: Option<Py<PyMarketImpact>>,
}

#[pymethods]
impl PyReport {
    #[getter]
    fn fills(&self, py: Python<'_>) -> Option<Vec<Py<PyMatchedFill>>> {
        self.fills
            .as_ref()
            .map(|fills| fills.iter().map(|fill| fill.clone_ref(py)).collect())
    }

    #[getter]
    fn summary(&self, py: Python<'_>) -> Option<Py<PySummary>> {
        self.summary.as_ref().map(|summary| summary.clone_ref(py))
    }

    #[getter]
    fn market_impact(&self, py: Python<'_>) -> Option<Py<PyMarketImpact>> {
        self.market_impact
            .as_ref()
            .map(|market_impact| market_impact.clone_ref(py))
    }

    #[classattr]
    fn __display_fields__() -> Vec<&'static str> {
        Self::display_fields()
    }
}

impl DisplayFields for PyReport {
    fn display_fields() -> Vec<&'static str> {
        vec!["fills", "summary", "market_impact"]
    }
}

impl PyReport {
    fn from_report(py: Python<'_>, report: Report<CompressedPrice>) -> PyResult<Py<Self>> {
        let fills = report
            .fills
            .map(|fills| {
                fills
                    .into_iter()
                    .map(|fill| PyMatchedFill::from(fill).into_python(py))
                    .collect()
            })
            .transpose()?;
        let summary = report
            .summary
            .map(|summary| PySummary::from(summary).into_python(py))
            .transpose()?;
        let market_impact = report
            .market_impact
            .map(|market_impact| PyMarketImpact::from(market_impact).into_python(py))
            .transpose()?;

        Py::new(
            py,
            PyClassInitializer::from(PyDisplay).add_subclass(Self {
                fills,
                summary,
                market_impact,
            }),
        )
    }
}

#[pyclass(name = "Fill", module = "lobo", extends=PyDisplay)]
pub struct PyFill {
    pub remaining_order_id: Option<Uuid>,
    report: Option<Py<PyReport>>,
}

#[pyclass(name = "Cancel", module = "lobo", get_all)]
pub struct PyCancel {
    pub order_id: Uuid,
}

#[pymethods]
impl PyFill {
    #[getter]
    fn remaining_order_id(&self) -> Option<Uuid> {
        self.remaining_order_id
    }

    #[getter]
    fn report(&self, py: Python<'_>) -> Option<Py<PyReport>> {
        self.report.as_ref().map(|report| report.clone_ref(py))
    }

    #[classattr]
    fn __display_fields__() -> Vec<&'static str> {
        Self::display_fields()
    }
}
impl DisplayFields for PyFill {
    fn display_fields() -> Vec<&'static str> {
        vec!["remaining_order_id", "report"]
    }
}

impl PyFill {
    fn from_command_result(
        py: Python<'_>,
        result: CommandResult<CompressedPrice>,
    ) -> PyResult<Self> {
        match result {
            CommandResult::Filled {
                remaining_order_id,
                report,
            } => Ok(Self {
                remaining_order_id,
                report: report
                    .map(|report| PyReport::from_report(py, report))
                    .transpose()?,
            }),

            _ => Err(PyRuntimeError::new_err("expected a Filled command result")),
        }
    }

    fn into_python(self, py: Python<'_>) -> PyResult<Py<Self>> {
        Py::new(py, PyClassInitializer::from(PyDisplay).add_subclass(self))
    }
}

impl TryFrom<CommandResult<CompressedPrice>> for PyCancel {
    type Error = PyErr;

    fn try_from(result: CommandResult<CompressedPrice>) -> Result<Self, Self::Error> {
        match result {
            CommandResult::Canceled { order_id } => Ok(Self { order_id }),

            _ => Err(PyRuntimeError::new_err("expected a Cancel command result")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lobo_storage::{OrderStateError, UserStateError};

    #[test]
    fn command_errors_map_to_python_runtime_errors() {
        pyo3::Python::initialize();
        pyo3::Python::attach(|py| {
            let user_error = command_error(CommandError::UserStateError(
                UserStateError::ConflictingStateError,
            ));
            let order_error = command_error(CommandError::OrderStateError(
                OrderStateError::OrderDoesNotExists,
            ));
            assert!(user_error.is_instance_of::<PyRuntimeError>(py));
            assert!(order_error.is_instance_of::<PyRuntimeError>(py));
        });
    }

    #[test]
    fn python_result_conversions_accept_only_matching_command_results() {
        pyo3::Python::initialize();
        let order_id = Uuid::new_v4();
        let cancel = PyCancel::try_from(CommandResult::Canceled { order_id }).unwrap();
        assert_eq!(cancel.order_id, order_id);

        pyo3::Python::attach(|py| {
            let fill = PyFill::from_command_result(
                py,
                CommandResult::Filled {
                    remaining_order_id: Some(order_id),
                    report: None,
                },
            )
            .unwrap();
            assert_eq!(fill.remaining_order_id, Some(order_id));
            assert!(fill.report.is_none());

            let fill_error = PyFill::from_command_result(py, CommandResult::Canceled { order_id })
                .err()
                .unwrap();
            let cancel_error = PyCancel::try_from(CommandResult::Added { order_id })
                .err()
                .unwrap();
            assert!(fill_error.is_instance_of::<PyRuntimeError>(py));
            assert!(cancel_error.is_instance_of::<PyRuntimeError>(py));
        });
    }

    #[test]
    fn python_display_field_contracts_are_stable() {
        assert_eq!(
            PyReports::__display_fields__(),
            ["include_fills", "include_summary", "include_market_impact"]
        );
        assert_eq!(
            PySummary::__display_fields__(),
            [
                "order_id",
                "trader_id",
                "side",
                "filled_quantity",
                "realized_price"
            ]
        );
        assert_eq!(
            PyMarketImpact::__display_fields__(),
            [
                "avg_price",
                "worst_price",
                "slippage",
                "slippage_bps",
                "levels_consumed",
                "total_quantity_available"
            ]
        );
        assert_eq!(
            PyMatchedFill::__display_fields__(),
            [
                "maker_order_id",
                "taker_order_id",
                "fill_quantity",
                "maker_depleted",
                "maker_trader_id",
                "price",
                "fill_time"
            ]
        );
        assert_eq!(
            PyReport::__display_fields__(),
            ["fills", "summary", "market_impact"]
        );
        assert_eq!(
            PyFill::__display_fields__(),
            ["remaining_order_id", "report"]
        );
        assert_eq!(PyStorage::__display_fields__(), ["bids", "asks"]);
        assert_eq!(
            PyBidAskStoreView::__display_fields__(),
            ["visible_quantity", "hidden_quantity", "order_count"]
        );
    }

    #[test]
    fn generated_python_book_constructor_uses_the_expected_native_book() {
        Python::initialize();
        Python::attach(|py| {
            let book = PyBook::py_new(py, None, true, lobo_models::CheckSum::Null, true).unwrap();
            assert_eq!(book.best_bid(), None);
            assert_eq!(book.best_ask(), None);
        });
    }
}
