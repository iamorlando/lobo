use lobo_books::price_time_priority::Book;
use lobo_primitives::Price64;
use lobo_replay::feed::FeedBook;
use lobo_storage::{
    DoNotUpdateUserMap, policies::DoNotUpdateHiddenQuantity, price_level::IntrusivePriceLevel,
    price_sorting::BTreeMapPriceSorting,
};
type Native<C = lobo_storage::policies::checksum::NoChecksum> = Book<
    IntrusivePriceLevel<Price64, DoNotUpdateHiddenQuantity, C>,
    BTreeMapPriceSorting,
    DoNotUpdateUserMap,
    DoNotUpdateHiddenQuantity,
    lobo_replay::feed::FeedPublishers,
>;
#[allow(dead_code)]
pub fn native(book: &FeedBook) -> &Native {
    let FeedBook::NoUsersNoHiddenNull(book) = book else {
        panic!("venue changed its native policy");
    };
    book
}
#[allow(dead_code)]
pub fn native_mut(book: &mut FeedBook) -> &mut Native {
    let FeedBook::NoUsersNoHiddenNull(book) = book else {
        panic!("venue changed its native policy");
    };
    book
}

#[allow(dead_code)]
pub fn kraken(book: &FeedBook) -> &Native<lobo_storage::policies::checksum::Kraken> {
    let FeedBook::NoUsersNoHiddenKraken(book) = book else { panic!("expected Kraken checksum policy"); }; book
}
#[allow(dead_code)]
pub fn bitfinex(book: &FeedBook) -> &Native<lobo_storage::policies::checksum::BitFinex> {
    let FeedBook::NoUsersNoHiddenBitFinex(book) = book else { panic!("expected Bitfinex checksum policy"); }; book
}
