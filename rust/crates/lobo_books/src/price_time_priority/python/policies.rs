//! Constructors are generated from the shared Cartesian policy matrix.
use super::*;
use lobo_models::CheckSum;
use lobo_storage::policies::checksum::ChecksumPolicy;
use std::{collections::HashMap, sync::LazyLock};
type Key = (bool, CheckSum, bool);
type Constructor = fn(Option<String>) -> PyBook;
fn construct<U, H, C>(id: Option<String>) -> PyBook
where
    U: UserMapUpdatePolicy + Send + Sync + 'static,
    H: HiddenQuantityPolicy + Send + Sync + 'static,
    C: ChecksumPolicy,
{
    let book = Book::<DeepPriceLevel<CompressedPrice, H, C>, SortedVectorPriceSorting, U, H>::new();
    PyBook::from(match id {
        Some(id) => book.with_id(id),
        None => book,
    })
}
macro_rules! constructors {
    (() [$(($u:ident, $ub:literal, $ut:ty, $h:ident, $hb:literal, $ht:ty, $c:ident, $ct:ty),)*]) => {
        static CONSTRUCTORS: LazyLock<HashMap<Key, Constructor>> = LazyLock::new(|| HashMap::from([
            $((($ub,CheckSum::$c,$hb), construct::<$ut,$ht,$ct> as Constructor),)*
        ]));
    };
}
lobo_storage::book_policy_matrix!(constructors);
pub(super) fn create(id: Option<String>, users: bool, checksum: CheckSum, hidden: bool) -> PyBook {
    CONSTRUCTORS[&(users, checksum, hidden)](id)
}
