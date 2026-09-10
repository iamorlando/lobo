use std::path::PathBuf;

use lobo_primitives::CompressedPrice;
use lobo_replay::ReplaySource;
use lobo_storage::{
    DoNotUpdateUserMap, policies::DoNotUpdateHiddenQuantity, price_level::IntrusivePriceLevel,
    price_sorting::SortedVectorPriceSorting,
};
#[cfg(feature = "python-polars")]
use polars::prelude::{Column, DataFrame, LazyFrame, col};

#[cfg(feature = "python-polars")] // unsused if not using python-polars
use pyo3::types::PyType;
use pyo3::{
    exceptions::{PyFileNotFoundError, PyRuntimeError},
    prelude::*,
};
#[cfg(feature = "python-polars")]
use pyo3_polars::{PyDataFrame, PyLazyFrame};

use super::{ItchReplayError, ItchReplaySource};

pub type ReplayLevel = IntrusivePriceLevel<CompressedPrice, DoNotUpdateHiddenQuantity>;
type NativeSource = ItchReplaySource<PathBuf>;
pub type NativeBook = <NativeSource as ReplaySource<
    ReplayLevel,
    SortedVectorPriceSorting,
    DoNotUpdateUserMap,
    DoNotUpdateHiddenQuantity,
>>::DefaultBookConcreteType;

pub use lobo_books::price_time_priority::python::{PyBidAskStoreView, PyBook, PyStorage};

/// Select an instrument from a NASDAQ ITCH replay file.
///
/// Pass this source to ReplayContext to reconstruct its book. The source can be
/// reused for subsequent replays. Use all_available() to select every instrument
/// in the file's directory with one shared input scan.
///
/// Args:
///     path: Filename containing length-prefixed ITCH messages, optionally gzip
///         compressed. Intraday timestamps use the session date in the filename.
///     ticker: Instrument symbol to reconstruct, such as "AAPL".
///
/// Raises:
///     FileNotFoundError: The input file does not exist.
///     RuntimeError: The file is unreadable or ticker is absent from its directory.
///
/// Examples:
///     ```python
///     from lobo.replay import ReplayContext
///     from lobo.replay.adapters import itch
///
///     source = itch.ItchSource("01302020.NASDAQ_ITCH50.gz", "AAPL")
///     with ReplayContext(source=source, cutoff_time=None) as books:
///         book = books["aapl"]
///     ```
#[pyclass(
    name = "ItchSource",
    module = "lobo.replay.adapters.itch",
    frozen,
    skip_from_py_object
)]
#[derive(Clone, Debug)]
pub struct PyItchSource {
    source: NativeSource,
}

#[pymethods]
impl PyItchSource {
    #[new]
    fn new(path: String, ticker: String) -> PyResult<Self> {
        let path = checked_path(path)?;
        Ok(Self {
            source: ItchReplaySource::from_file(path, ticker).map_err(replay_error)?,
        })
    }
    /// Select every instrument present in the file's directory.
    ///
    /// Args:
    ///     path: The ITCH replay filename, optionally gzip compressed.
    ///
    /// Returns:
    ///     Sources sharing the same input scan, one per available ticker.
    ///
    /// Raises:
    ///     FileNotFoundError: The input file does not exist.
    ///     RuntimeError: Reading or parsing the directory fails.
    #[staticmethod]
    fn all_available(path: String) -> PyResult<Vec<Self>> {
        let path = checked_path(path)?;
        ItchReplaySource::from_file_all(path)
            .map(|sources| sources.into_iter().map(|source| Self { source }).collect())
            .map_err(replay_error)
    }

    #[getter]
    fn path(&self) -> String {
        self.source.path().to_string_lossy().into_owned()
    }

    #[getter]
    fn ticker(&self) -> &str {
        self.source.ticker()
    }

    fn __fspath__(&self) -> String {
        self.path()
    }

    fn __repr__(&self) -> String {
        format!(
            "ItchSource(path={:?}, ticker={:?})",
            self.path(),
            self.source.ticker(),
        )
    }
}

#[cfg(feature = "python-polars")]
#[pymethods]
impl PyItchSource {
    /// Read the file's ticker directory into a Polars DataFrame.
    #[staticmethod]
    fn read_tickers(path: String) -> PyResult<PyDataFrame> {
        let path = checked_path(path)?;
        let tickers = ItchReplaySource::from_file_tickers(path).map_err(replay_error)?;
        let mut rows: Vec<_> = tickers.into_iter().collect();
        rows.sort_unstable_by_key(|(_, stock_locate)| *stock_locate);

        let (tickers, stock_locates): (Vec<_>, Vec<_>) = rows
            .into_iter()
            .map(|(ticker, stock_locate)| (ticker, u32::from(stock_locate)))
            .unzip();
        DataFrame::new_infer_height(vec![
            Column::new("ticker".into(), tickers),
            Column::new("stock_locate".into(), stock_locates),
        ])
        .map(PyDataFrame)
        .map_err(|error| PyRuntimeError::new_err(error.to_string()))
    }

    /// Return a Polars LazyFrame containing every source message.
    fn to_lazy_frame(&self) -> PyResult<PyLazyFrame> {
        self.native_lazy_frame().map(PyLazyFrame)
    }

    /// Return only messages that can update a reconstructed order book.
    fn adapt(&self) -> PyResult<PyReplayMessages> {
        let lazy_frame = self
            .native_lazy_frame()?
            .filter(col("operation").is_not_null());
        Ok(PyReplayMessages::new(lazy_frame))
    }
}

impl PyItchSource {
    pub fn native(&self) -> &NativeSource {
        &self.source
    }

    #[cfg(feature = "python-polars")]
    fn native_lazy_frame(&self) -> PyResult<LazyFrame> {
        <NativeSource as ReplaySource<
            ReplayLevel,
            SortedVectorPriceSorting,
            DoNotUpdateUserMap,
            DoNotUpdateHiddenQuantity,
        >>::to_lazy_frame(&self.source)
        .map_err(replay_error)
    }
}

/// A replay-compatible native Polars lazy plan.
#[cfg(feature = "python-polars")]
#[pyclass(name = "ReplayMessages", module = "lobo.replay.adapters.itch", frozen)]
pub struct PyReplayMessages {
    lazy_frame: LazyFrame,
}

#[cfg(feature = "python-polars")]
impl PyReplayMessages {
    pub fn new(lazy_frame: LazyFrame) -> Self {
        Self { lazy_frame }
    }
}

#[cfg(feature = "python-polars")]
#[pymethods]
impl PyReplayMessages {
    #[classmethod]
    fn _from_lazy_frame(_cls: &Bound<'_, PyType>, lazy_frame: PyLazyFrame) -> PyReplayMessages {
        Self::new(lazy_frame.0)
    }

    fn to_lazy_frame(&self) -> PyLazyFrame {
        PyLazyFrame(self.lazy_frame.clone())
    }

    fn __repr__(&self) -> &'static str {
        "ReplayMessages(<Polars LazyFrame>)"
    }
}

fn replay_error(error: ItchReplayError) -> PyErr {
    PyRuntimeError::new_err(error.to_string())
}

fn checked_path(path: String) -> PyResult<PathBuf> {
    let path = PathBuf::from(path);
    if path.is_file() {
        Ok(path)
    } else {
        Err(PyFileNotFoundError::new_err(format!(
            "ITCH data file does not exist: {}",
            path.display()
        )))
    }
}
