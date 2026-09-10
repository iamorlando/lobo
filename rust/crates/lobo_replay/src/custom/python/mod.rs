//! Python declarations and adapter bindings.
#![warn(missing_docs)]
mod adapter;
mod bulk;
mod session;
use super::{AdapterDescriptor, BookLevel, FeedMode};
pub use adapter::{PyCustomAdapter, PySource};
use lobo_models::BookPolicy;
use pyo3::{exceptions::PyValueError, prelude::*};
pub use session::PyAdapterSession;

// Consume setup iterables once, including generators, and describe their element
// type to the existing PyO3 stub generator. This is not a packet callback.
pub(super) struct Iterable<T>(pub(super) Vec<T>);
impl<'py, T: pyo3::conversion::FromPyObjectOwned<'py>> FromPyObject<'_, 'py> for Iterable<T> {
    type Error = PyErr;
    const INPUT_TYPE: pyo3::inspect::PyStaticExpr = pyo3::type_hint_subscript!(
        pyo3::type_hint_identifier!("collections.abc", "Iterable"),
        T::INPUT_TYPE
    );
    fn extract(value: Borrowed<'_, 'py, PyAny>) -> PyResult<Self> {
        value
            .try_iter()?
            .map(|item| item?.extract::<T>().map_err(Into::into))
            .collect::<PyResult<Vec<_>>>()
            .map(Self)
    }
}
impl<T> Iterable<T> {
    pub(super) fn or_empty(value: Option<Self>) -> Vec<T> {
        value.map_or_else(Vec::new, |items| items.0)
    }
}

// A declaration owns a copy, while the Python input is a read-only Mapping.
// Its value type is covariant, so dictionaries of UInt/Text are valid fields.
pub(super) struct Mapping<K, V>(pub(super) std::collections::BTreeMap<K, V>);
impl<'py, K, V> FromPyObject<'_, 'py> for Mapping<K, V>
where
    K: pyo3::conversion::FromPyObjectOwned<'py> + Ord,
    V: pyo3::conversion::FromPyObjectOwned<'py>,
{
    type Error = PyErr;
    const INPUT_TYPE: pyo3::inspect::PyStaticExpr = pyo3::type_hint_subscript!(
        pyo3::type_hint_identifier!("collections.abc", "Mapping"),
        K::INPUT_TYPE,
        V::INPUT_TYPE
    );
    fn extract(value: Borrowed<'_, 'py, PyAny>) -> PyResult<Self> {
        use pyo3::types::PyMappingMethods;
        value
            .cast::<pyo3::types::PyMapping>()?
            .items()?
            .iter()
            .map(|item| item.extract::<(K, V)>())
            .collect::<PyResult<_>>()
            .map(Self)
    }
}

pub(crate) fn policy(value: &str) -> PyResult<BookPolicy> {
    match value {
        "full" => Ok(BookPolicy::Full),
        "no_user_map" => Ok(BookPolicy::NoUserMap),
        "no_hidden_quantity" => Ok(BookPolicy::NoHiddenQuantity),
        "no_updates" => Ok(BookPolicy::NoUpdates),
        _ => Err(PyValueError::new_err(
            "policy must be full, no_user_map, no_hidden_quantity, or no_updates",
        )),
    }
}
/// Describe one instrument's symbol, decimal precision, and book policy.
///
/// Pass an iterable of Instrument objects to CustomAdapter when this information is
/// known before the source starts. A generator is consumed once during construction.
/// Sources that publish instrument metadata can use Register actions instead.
///
/// Args:
///     symbol: The ticker used to identify and select this book. Surrounding whitespace
///         is removed and letters are normalized to uppercase.
///     price_decimals: Number of fractional price digits. With two digits, an integer
///         action price of 12345 represents 123.45.
///     quantity_decimals: Number of fractional quantity digits. With three digits,
///         an integer action quantity of 1250 represents 1.250 units.
///     book_policy: "full" maintains user lookup and hidden quantities; "no_user_map"
///         maintains hidden quantities only; "no_hidden_quantity" maintains user lookup
///         only; "no_updates" maintains neither and is the default. Hidden-quantity
///         policies are required to retain iceberg reserve quantities. This choice is
///         applied when constructing the book, before messages are processed.
///
/// Raises:
///     ValueError: The symbol is invalid or book_policy is not a supported policy name.
///     OverflowError: A decimal count is outside the unsigned-byte range.
///
/// Examples:
///     ```python
///     from lobo.replay.adapters import models as lm
///
///     instrument = lm.Instrument("AAPL", price_decimals=4, quantity_decimals=0)
///     ```
#[pyclass(
    name = "Instrument",
    module = "lobo.replay.adapters.models",
    frozen,
    from_py_object
)]
#[derive(Clone)]
pub struct PyInstrument {
    pub(crate) native: crate::feed::Instrument,
    pub(crate) policy: BookPolicy,
}
#[pymethods]
impl PyInstrument {
    #[new]
    #[pyo3(signature=(symbol,price_decimals,quantity_decimals,*,book_policy="no_updates"))]
    fn new(
        symbol: &str,
        price_decimals: u8,
        quantity_decimals: u8,
        book_policy: &str,
    ) -> PyResult<Self> {
        Ok(Self {
            native: crate::feed::Instrument {
                symbol: crate::feed::normalize_symbol(symbol).map_err(PyValueError::new_err)?,
                price_decimals,
                quantity_decimals,
            },
            policy: policy(book_policy)?,
        })
    }
    /// The normalized instrument symbol used for book routing.
    #[getter]
    fn symbol(&self) -> &str {
        &self.native.symbol
    }
    /// The number of fractional digits represented by integer price atoms.
    #[getter]
    fn price_decimals(&self) -> u8 {
        self.native.price_decimals
    }
    /// The number of fractional digits represented by integer quantity atoms.
    #[getter]
    fn quantity_decimals(&self) -> u8 {
        self.native.quantity_decimals
    }
    /// The policy selected for user lookup and hidden-quantity maintenance.
    #[getter]
    fn book_policy(&self) -> &str {
        match self.policy {
            BookPolicy::Full => "full",
            BookPolicy::NoUserMap => "no_user_map",
            BookPolicy::NoHiddenQuantity => "no_hidden_quantity",
            BookPolicy::NoUpdates => "no_updates",
        }
    }
}
/// Describe a source's identity and the capabilities its consumers may use.
///
/// The adapter exposes equivalent information through CustomAdapter.info. Mode and
/// level determine which replay, queue, and simulation controls a consumer can offer.
/// This metadata does not define how messages are decoded; Protocol does that.
///
/// Args:
///     id: A stable identifier for the source, such as "my_feed".
///     name: A human-readable source name shown to consumers.
///     mode: FeedMode.Live for ongoing updates or FeedMode.Replay for a recorded session.
///     level: BookLevel.L1 for best quotes, BookLevel.L2 for aggregate price levels,
///         or BookLevel.L3 for individual resting orders.
///     default_symbol: The initially selected instrument symbol.
///     endpoint: The source's WebSocket URL, if it has one. Defaults to None.
///     timezone: An IANA timezone name describing source timestamps, such as
///         "America/New_York". Defaults to "UTC".
///     supports_trades: Whether the protocol reports actual executions through Execute,
///         Trade, or the order API. Defaults to True; quantity changes alone do not
///         imply executions and should not advertise trade reporting.
///
/// Raises:
///     TypeError: mode or level is not the corresponding enum.
///     ValueError: default_symbol is invalid.
///
/// Examples:
///     ```python
///     from lobo.replay.adapters import models as lm
///
///     info = lm.AdapterInfo("my_feed", "My feed", mode=lm.FeedMode.Live, level=lm.BookLevel.L2,
///                        default_symbol="BTC/USD", supports_trades=True)
///     ```
#[pyclass(
    name = "AdapterInfo",
    module = "lobo.replay.adapters.models",
    frozen,
    from_py_object
)]
#[derive(Clone)]
pub struct PyAdapterInfo {
    pub(crate) descriptor: AdapterDescriptor,
}
#[pymethods]
impl PyAdapterInfo {
    #[new]
    #[pyo3(signature=(id,name,*,mode,level,default_symbol,endpoint=None,timezone="UTC",supports_trades=true))]
    fn new(
        id: String,
        name: String,
        mode: FeedMode,
        level: BookLevel,
        default_symbol: &str,
        endpoint: Option<String>,
        timezone: &str,
        supports_trades: bool,
    ) -> PyResult<Self> {
        Ok(Self {
            descriptor: AdapterDescriptor {
                id,
                name,
                mode,
                level,
                default_symbol: crate::feed::normalize_symbol(default_symbol)
                    .map_err(PyValueError::new_err)?,
                endpoint,
                timezone: timezone.into(),
                supports_trades,
            },
        })
    }
    /// The stable source identifier.
    #[getter]
    fn id(&self) -> &str {
        &self.descriptor.id
    }
    /// The display name of the source.
    #[getter]
    fn name(&self) -> &str {
        &self.descriptor.name
    }
    /// FeedMode.Live for ongoing updates or FeedMode.Replay for recorded input.
    #[getter]
    fn mode(&self) -> FeedMode {
        self.descriptor.mode
    }
    /// The source's BookLevel.L1, BookLevel.L2, or BookLevel.L3 detail.
    #[getter]
    fn level(&self) -> BookLevel {
        self.descriptor.level
    }
    /// The instrument selected when the adapter starts.
    #[getter]
    fn default_symbol(&self) -> &str {
        &self.descriptor.default_symbol
    }
    /// The source WebSocket URL, or None if no endpoint is declared.
    #[getter]
    fn endpoint(&self) -> Option<&str> {
        self.descriptor.endpoint.as_deref()
    }
    /// The source timezone name used to interpret recorded timestamps.
    #[getter]
    fn timezone(&self) -> &str {
        &self.descriptor.timezone
    }
    /// Whether actual executions are reported by the declared protocol.
    #[getter]
    fn supports_trades(&self) -> bool {
        self.descriptor.supports_trades
    }
}

/// AUTOGENERATED FROM RUST. DO NOT EDIT.
///
/// Connect a user-defined protocol to its source with CustomAdapter.
#[pymodule]
#[pyo3(name = "custom", submodule, module = "lobo.replay.adapters")]
pub mod native {
    #[pymodule_export]
    use super::PyCustomAdapter;
}
