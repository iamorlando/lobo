//! One Python type for concrete book policies and context-owned books.
use super::*;
use lobo_models::{BookPolicy, CheckSum};
use lobo_storage::policies::checksum::ChecksumPolicy;

/// Erasure occurs once per Python call. Matching and publication remain generic.
pub trait BookAccess: Send + Sync {
    fn disconnect(&mut self);
    fn policy(&self) -> BookPolicy;
    fn checksum_spec(&self) -> CheckSum;
    fn checksum(&mut self) -> PyResult<u32>;
    fn best(&self, side: Side) -> Option<CompressedPrice>;
    fn visible(&self, side: Side) -> u64;
    fn hidden(&self, side: Side) -> u64;
    fn len(&self, side: Side) -> usize;
    fn summary(&mut self, py: Python<'_>, trader: Uuid) -> PyResult<Py<UserOutstandingLiquidity>>;
    fn fill(
        &mut self,
        order: Bound<'_, PyOrder>,
        reports: Option<PyRef<'_, PyReports>>,
        execution: PyExecution,
    ) -> PyResult<Py<PyFill>>;
    fn cancel(&mut self, id: Uuid) -> PyResult<PyCancel>;
    #[cfg(feature = "python-polars")]
    fn levels(&self, py: Python<'_>, side: Side, rows: Option<usize>) -> PyResult<Py<PyAny>>;
}
impl<L, S, U, H> BookAccess for PythonBook<L, S, U, H>
where
    L: PriceLevelContract<Price = CompressedPrice> + Send + Sync,
    S: PriceSortingPolicy + Send + Sync,
    U: UserMapUpdatePolicy + Send + Sync,
    H: HiddenQuantityPolicy + Send + Sync,
    Book<L, S, U, H>: Send + Sync,
    Book<L, S, U, H, MpscPublisher<PriceLevelChangeEvent<CompressedPrice>>>: Send + Sync,
{
    fn disconnect(&mut self) {
        PythonBook::disconnect(self);
    }
    fn policy(&self) -> BookPolicy {
        BookPolicy::from_flags(U::ENABLED, H::ENABLED)
    }
    fn checksum_spec(&self) -> CheckSum {
        L::Checksum::SPEC
    }
    fn checksum(&mut self) -> PyResult<u32> {
        crate::dispatch_python_book!(
            self,
            book,
            book.order_storage
                .checksum()
                .map_err(PyRuntimeError::new_err)
        )
    }
    fn best(&self, side: Side) -> Option<CompressedPrice> {
        crate::dispatch_python_book!(
            self,
            book,
            match side {
                Side::Buy => py_best_bid(book),
                Side::Sell => py_best_ask(book),
            }
        )
    }
    fn visible(&self, side: Side) -> u64 {
        crate::dispatch_python_book!(self, book, py_visible_quantity(book, side))
    }
    fn hidden(&self, side: Side) -> u64 {
        crate::dispatch_python_book!(self, book, py_hidden_quantity(book, side))
    }
    fn len(&self, side: Side) -> usize {
        crate::dispatch_python_book!(self, book, py_order_count(book, side))
    }
    fn summary(&mut self, py: Python<'_>, trader: Uuid) -> PyResult<Py<UserOutstandingLiquidity>> {
        crate::dispatch_python_book!(self, book, py_user_order_summary(book, py, trader))
    }
    fn fill(
        &mut self,
        order: Bound<'_, PyOrder>,
        reports: Option<PyRef<'_, PyReports>>,
        execution: PyExecution,
    ) -> PyResult<Py<PyFill>> {
        if let Self::Hosted(book) = self {
            return hosted::fill(book.commands.clone(), order, reports, execution);
        }
        crate::dispatch_python_book!(self, book, py_fill(book, order, reports, execution))
    }
    fn cancel(&mut self, id: Uuid) -> PyResult<PyCancel> {
        if let Self::Hosted(book) = self {
            return book
                .commands
                .apply(
                    lobo_models::server::Command::Remove { id },
                    Reports::default(),
                )
                .map_err(PyRuntimeError::new_err)?
                .try_into();
        }
        crate::dispatch_python_book!(self, book, py_cancel(book, id))
    }
    #[cfg(feature = "python-polars")]
    fn levels(&self, py: Python<'_>, side: Side, rows: Option<usize>) -> PyResult<Py<PyAny>> {
        crate::dispatch_python_book!(self, book, py_levels_df(book, py, side, rows))
    }
}

#[pyclass(name = "Book", module = "lobo")]
pub struct PyBook {
    pub book: Box<dyn BookAccess>,
}

impl<L, S, U, H> From<Book<L, S, U, H>> for PyBook
where
    L: PriceLevelContract,
    S: PriceSortingPolicy,
    U: UserMapUpdatePolicy,
    H: HiddenQuantityPolicy,
    PythonBook<L, S, U, H>: BookAccess + 'static,
{
    fn from(book: Book<L, S, U, H>) -> Self {
        Self {
            book: Box::new(PythonBook::Null(book)),
        }
    }
}
impl<L, S, U, H> From<Book<L, S, U, H, MpscPublisher<PriceLevelChangeEvent<L::Price>>>> for PyBook
where
    L: PriceLevelContract,
    S: PriceSortingPolicy,
    U: UserMapUpdatePolicy,
    H: HiddenQuantityPolicy,
    PythonBook<L, S, U, H>: BookAccess + 'static,
{
    fn from(book: Book<L, S, U, H, MpscPublisher<PriceLevelChangeEvent<L::Price>>>) -> Self {
        Self {
            book: Box::new(PythonBook::Publishing(book)),
        }
    }
}
impl<L, S, U, H> From<hosted::HostedBook<L, S, U, H>> for PyBook
where
    L: PriceLevelContract,
    S: PriceSortingPolicy,
    U: UserMapUpdatePolicy,
    H: HiddenQuantityPolicy,
    PythonBook<L, S, U, H>: BookAccess + 'static,
{
    fn from(book: hosted::HostedBook<L, S, U, H>) -> Self {
        Self {
            book: Box::new(PythonBook::Hosted(book)),
        }
    }
}

#[pymethods]
impl PyBook {
    #[new]
    #[pyo3(signature = (book_id=None, *, update_user_map=true, check_sum_spec=CheckSum::Null, update_hidden=true))]
    pub(super) fn py_new(
        py: Python<'_>,
        book_id: Option<String>,
        update_user_map: bool,
        check_sum_spec: CheckSum,
        update_hidden: bool,
    ) -> PyResult<Self> {
        if let Some(book) = hosted::create_book(
            py,
            book_id.clone(),
            BookPolicy::from_flags(update_user_map, update_hidden),
            check_sum_spec,
        )? {
            return Ok(book);
        }
        Ok(policies::create(
            book_id,
            update_user_map,
            check_sum_spec,
            update_hidden,
        ))
    }
    #[getter]
    fn update_user_map(&self) -> bool {
        self.book.policy().update_user_map()
    }
    #[getter]
    fn update_hidden(&self) -> bool {
        self.book.policy().update_hidden()
    }
    #[getter]
    fn check_sum_spec(&self) -> CheckSum {
        self.book.checksum_spec()
    }
    /// Finish the checksum over the current book after a complete message.
    fn checksum(&mut self) -> PyResult<u32> {
        self.book.checksum()
    }
    #[getter]
    fn orders(slf: PyRef<'_, Self>) -> PyResult<Py<PyStorage>> {
        let py = slf.py();
        let book = slf.into();
        PyStorage::new(book).into_python(py)
    }
    fn get_user_order_summary(
        &mut self,
        py: Python<'_>,
        trader: Uuid,
    ) -> PyResult<Py<UserOutstandingLiquidity>> {
        self.book.summary(py, trader)
    }
    #[pyo3(signature = (order, reports=None, execution=PyExecution::Mutating))]
    fn fill(
        &mut self,
        order: Bound<'_, PyOrder>,
        reports: Option<PyRef<'_, PyReports>>,
        execution: PyExecution,
    ) -> PyResult<Py<PyFill>> {
        self.book.fill(order, reports, execution)
    }
    fn cancel(&mut self, order_id: Uuid) -> PyResult<PyCancel> {
        self.book.cancel(order_id)
    }
    #[getter]
    pub(super) fn best_bid(&self) -> Option<CompressedPrice> {
        self.book.best(Side::Buy)
    }
    #[getter]
    pub(super) fn best_ask(&self) -> Option<CompressedPrice> {
        self.book.best(Side::Sell)
    }
}

#[pyclass(module="lobo.storage", extends=PyDisplay, name="OrderStorage")]
pub struct PyStorage {
    book: Py<PyBook>,
}
impl PyStorage {
    pub fn new(book: Py<PyBook>) -> Self {
        Self { book }
    }
    pub fn into_python(self, py: Python<'_>) -> PyResult<Py<Self>> {
        Py::new(py, PyClassInitializer::from(PyDisplay).add_subclass(self))
    }
}
#[pyclass(module="lobo.storage", extends=PyDisplay, name="BidAskStore")]
pub struct PyBidAskStoreView {
    book: Py<PyBook>,
    side: Side,
}
impl PyBidAskStoreView {
    pub fn new(book: Py<PyBook>, side: Side) -> Self {
        Self { book, side }
    }
    pub fn into_python(self, py: Python<'_>) -> PyResult<Py<Self>> {
        Py::new(py, PyClassInitializer::from(PyDisplay).add_subclass(self))
    }
}
#[pymethods]
impl PyStorage {
    #[getter]
    fn bids(&self, py: Python<'_>) -> PyResult<Py<PyBidAskStoreView>> {
        PyBidAskStoreView::new(self.book.clone_ref(py), Side::Buy).into_python(py)
    }
    #[getter]
    fn asks(&self, py: Python<'_>) -> PyResult<Py<PyBidAskStoreView>> {
        PyBidAskStoreView::new(self.book.clone_ref(py), Side::Sell).into_python(py)
    }
    #[classattr]
    pub(super) fn __display_fields__() -> Vec<&'static str> {
        vec!["bids", "asks"]
    }
}
#[pymethods]
impl PyBidAskStoreView {
    #[getter]
    fn visible_quantity(&self, py: Python<'_>) -> PyResult<u64> {
        Ok(self.book.try_borrow(py)?.book.visible(self.side))
    }
    #[getter]
    fn hidden_quantity(&self, py: Python<'_>) -> PyResult<u64> {
        Ok(self.book.try_borrow(py)?.book.hidden(self.side))
    }
    #[getter]
    fn order_count(&self, py: Python<'_>) -> PyResult<usize> {
        Ok(self.book.try_borrow(py)?.book.len(self.side))
    }
    #[cfg(feature = "python-polars")]
    fn _levels_df(&self, py: Python<'_>, n_rows: Option<usize>) -> PyResult<Py<PyAny>> {
        self.book.try_borrow(py)?.book.levels(py, self.side, n_rows)
    }
    #[classattr]
    pub(super) fn __display_fields__() -> Vec<&'static str> {
        vec!["visible_quantity", "hidden_quantity", "order_count"]
    }
}
