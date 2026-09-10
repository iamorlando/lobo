use lobo_events::{NullPublisher, PublisherFactory};
use std::sync::Arc;
#[cfg(feature = "python")]
pub mod python;

#[cfg(feature = "python")]
pub use python::*;

use lobo_models::{
    events::{Report, Reports},
    orders::{
        core::Order,
        traits::{HandlesCompletion, IntoRestingOrderData},
    },
};
use lobo_primitives::{PriceType, uuid::Uuid};
use lobo_storage::{
    ExecutionPolicy, OrderStateError, OrderStorage, UpdateUserMap, UserMapUpdatePolicy,
    UserStateError,
    policies::{HiddenQuantityPolicy, UpdateHiddenQuantity},
    price_level::PriceLevelContract,
    price_sorting::{BTreeMapPriceSorting, PriceSortingPolicy},
};

#[cfg(test)]
use lobo_storage::MutatingFills;

use super::BookEvent;

/// A book whose price representation is selected by `L::Price`.
///
/// There is deliberately no default price type. For example,
/// `Book<DeepPriceLevel<Price64>, BTreeMapPriceSorting>` and
/// `Book<DeepPriceLevel<Price128>, BTreeMapPriceSorting>` are distinct books.
use super::Sequencer;

// #[derive(Default)]
pub struct Book<
    // Decides the data structure used to maintain FIFO within a price level.
    // Intrusive stores head/tail keys and links orders through the arena. Deep
    // stores keys in a VecDeque owned by each price level.
    L: PriceLevelContract,
    // Controls how price levels are indexed and sorted.
    Sort: PriceSortingPolicy = BTreeMapPriceSorting,
    U: UserMapUpdatePolicy = UpdateUserMap,
    H: HiddenQuantityPolicy = UpdateHiddenQuantity,
    Pub = NullPublisher,
> {
    pub order_storage: OrderStorage<L, Sort, U, H>,
    sequencer: Sequencer,
    book_id: Arc<str>,
    publisher: Pub,
}

impl<L, Sort, U, H, Pub> Default for Book<L, Sort, U, H, Pub>
where
    L: PriceLevelContract,
    Sort: PriceSortingPolicy,
    U: UserMapUpdatePolicy,
    H: HiddenQuantityPolicy,
    Pub: lobo_events::BookPublisherFactory<L::Price>,
{
    fn default() -> Self {
        Self {
            order_storage: OrderStorage::default(),
            sequencer: Sequencer::default(),
            book_id: Uuid::new_v4().to_string().into(),
            publisher: Pub::default(),
        }
    }
}

struct BookMutationPublisher<'a, Pub> {
    publisher: &'a Pub,
    sequencer: &'a mut Sequencer,
    book_id: &'a Arc<str>,
    timestamp_ns: u64,
}
impl<T, Pub: PublisherFactory<T>> lobo_events::EventPublisher<T>
    for BookMutationPublisher<'_, Pub>
{
    #[inline(always)]
    fn observe<R: Default>(&self, build: impl FnOnce() -> R) -> R {
        self.publisher.observe(build)
    }
    #[inline(always)]
    fn emit_with(&mut self, build: impl FnOnce() -> Option<T>) {
        self.publisher.observe(|| {
            if let Some(event) = build() {
                self.sequencer.next();
                self.publisher.send(
                    BookEvent::from_event(
                        event,
                        self.sequencer.current(),
                        Arc::clone(self.book_id),
                    )
                    .at_timestamp(self.timestamp_ns),
                );
            }
        });
    }
}

pub enum Command<T, R, E, PT: PriceType>
where
    Order<T, PT>: HandlesCompletion<PT>,
    R: IntoRestingOrderData,
    E: ExecutionPolicy,
{
    Fill {
        order: Order<T, PT>,
        execution: E,
        reports: Reports,
    },
    Add {
        order: Order<R, PT>,
    },
    Cancel {
        order_id: Uuid,
    },
    // Modify { order_id: Uuid },
    CancelAllForUser {
        user_id: Uuid,
    },
}
pub enum CommandResult<PT: PriceType> {
    Added {
        order_id: Uuid,
    },
    Filled {
        remaining_order_id: Option<Uuid>,
        report: Option<Report<PT>>,
    },
    Canceled {
        order_id: Uuid,
    },
    CanceledAllForUser {
        user_id: Uuid,
    },
}
pub enum CommandError {
    UserStateError(UserStateError),
    OrderStateError(OrderStateError),
}
impl From<UserStateError> for CommandError {
    fn from(value: UserStateError) -> Self {
        CommandError::UserStateError(value)
    }
}
impl From<OrderStateError> for CommandError {
    fn from(value: OrderStateError) -> Self {
        CommandError::OrderStateError(value)
    }
}
impl<L, Sort, U, H, Pub> Book<L, Sort, U, H, Pub>
where
    L: PriceLevelContract,
    Sort: PriceSortingPolicy,
    U: UserMapUpdatePolicy,
    H: HiddenQuantityPolicy,
    Pub: lobo_events::BookPublisherFactory<L::Price>,
{
    /// Stamp a typed message with this book's identity and monotonic sequence.
    #[inline(always)]
    pub fn publish<T>(&mut self, message: T)
    where
        Pub: PublisherFactory<T>,
    {
        let Self {
            sequencer,
            book_id,
            publisher,
            ..
        } = self;
        <Pub as PublisherFactory<T>>::publisher(publisher, |message: T| {
            sequencer.next();
            <Pub as PublisherFactory<T>>::send(
                publisher,
                BookEvent::from_event(message, sequencer.current(), Arc::clone(book_id)),
            );
        })(message);
    }

    /// Split the storage borrow from the already-connected book publisher.
    /// The null factory discards the stamping closure at compile time.
    #[inline(always)]
    pub fn storage_and_publisher(
        &mut self,
    ) -> (
        &mut OrderStorage<L, Sort, U, H>,
        impl lobo_events::MutationPublisher<L::Price> + '_,
    ) {
        self.storage_and_publisher_at(0)
    }

    /// Supply source time once for this mutation. Disabled routes discard the
    /// entire stamping closure, including its timestamp, through static dispatch.
    #[inline(always)]
    pub fn storage_and_publisher_at(
        &mut self,
        timestamp_ns: u64,
    ) -> (
        &mut OrderStorage<L, Sort, U, H>,
        impl lobo_events::MutationPublisher<L::Price> + '_,
    ) {
        let Self {
            order_storage,
            sequencer,
            book_id,
            publisher,
        } = self;
        let publish = BookMutationPublisher {
            publisher,
            sequencer,
            book_id,
            timestamp_ns,
        };
        (order_storage, publish)
    }

    pub fn with_publisher<Q>(self, publisher: Q) -> Book<L, Sort, U, H, Q> {
        Book {
            order_storage: self.order_storage,
            sequencer: self.sequencer,
            book_id: self.book_id,
            publisher,
        }
    }

    /// Copy native storage at a message boundary and connect independent sinks.
    /// Arena keys and FIFO links remain valid within the copied arena.
    pub fn fork_with_publisher<Q>(&self, publisher: Q) -> Book<L, Sort, U, H, Q>
    where
        OrderStorage<L, Sort, U, H>: Clone,
    {
        Book {
            order_storage: self.order_storage.clone(),
            sequencer: Sequencer {
                value: self.sequencer.current(),
            },
            book_id: Arc::clone(&self.book_id),
            publisher,
        }
    }

    pub fn set_publisher(&mut self, publisher: Pub) {
        self.publisher = publisher;
    }

    pub fn book_id(&self) -> &str {
        &self.book_id
    }
    pub fn sequence(&self) -> u64 {
        self.sequencer.current()
    }

    pub fn new() -> Self {
        Self::default()
    }
    pub fn with_id(self, book_id: String) -> Self {
        Self {
            book_id: book_id.into(),
            ..self
        }
    }

    pub fn submit<T, R, E>(
        &mut self,
        command: Command<T, R, E, L::Price>,
    ) -> Result<CommandResult<L::Price>, CommandError>
    where
        Order<T, L::Price>: HandlesCompletion<L::Price>,
        R: IntoRestingOrderData,
        E: ExecutionPolicy,
    {
        let (storage, mut publish) = self.storage_and_publisher();
        Ok(match command {
            Command::Add { order } => CommandResult::Added {
                order_id: storage.add_order(order, &mut publish)?,
            },
            Command::Cancel { order_id } => CommandResult::Canceled {
                order_id: storage.remove_order(order_id, &mut publish)?,
            },
            Command::CancelAllForUser { user_id } => CommandResult::CanceledAllForUser {
                user_id: storage.remove_user_orders(user_id, &mut publish)?,
            },

            Command::Fill {
                order,
                execution,
                reports,
            } => {
                let result =
                    storage.submit_order_with_reports(order, execution, reports, &mut publish);

                CommandResult::Filled {
                    remaining_order_id: result.remaining_order_id,
                    report: result.report,
                }
            }
        })
    }
}
#[cfg(test)]
mod result_and_error_tests {
    use super::*;
    use lobo_models::{
        Side,
        orders::{
            order_types::{LimitOrder, LimitOrderData},
            traits::Trades,
        },
    };
    use lobo_primitives::CompressedPrice;
    use lobo_storage::{price_level::DeepPriceLevel, price_sorting::BTreeMapPriceSorting};

    fn new_book()
    -> Book<DeepPriceLevel<CompressedPrice, UpdateHiddenQuantity>, BTreeMapPriceSorting> {
        Book::default()
    }

    #[test]
    fn add_cancel_and_cancel_all_commands_return_typed_results() {
        let mut book = new_book();
        let trader = Uuid::new_v4();
        let order = LimitOrder::new(Some(100), 3, trader, Side::Buy);
        let order_id = order.uuid();

        let added = book
            .submit::<LimitOrderData, LimitOrderData, MutatingFills>(Command::Add { order })
            .ok()
            .unwrap();
        assert!(matches!(
            added,
            CommandResult::Added { order_id: id } if id == order_id
        ));

        let canceled = book
            .submit::<LimitOrderData, LimitOrderData, MutatingFills>(Command::Cancel { order_id })
            .ok()
            .unwrap();
        assert!(matches!(
            canceled,
            CommandResult::Canceled { order_id: id } if id == order_id
        ));

        let canceled_all = book
            .submit::<LimitOrderData, LimitOrderData, MutatingFills>(Command::CancelAllForUser {
                user_id: trader,
            })
            .ok()
            .unwrap();
        assert!(matches!(
            canceled_all,
            CommandResult::CanceledAllForUser { user_id } if user_id == trader
        ));
    }

    #[test]
    fn storage_errors_convert_to_command_errors() {
        assert!(matches!(
            CommandError::from(UserStateError::ConflictingStateError),
            CommandError::UserStateError(_)
        ));
        assert!(matches!(
            CommandError::from(OrderStateError::OrderDoesNotExists),
            CommandError::OrderStateError(_)
        ));
    }
}
#[cfg(test)]
mod tests {
    use lobo_models::{
        Side,
        events::{MarketImpact, OrderDetails, Report, Reports, Summary},
        orders::{
            core::RestingOrder,
            order_types::{
                IcebergOrder, IcebergOrderData, LimitOrder, LimitOrderData, MarketOrder,
                MarketOrderData,
            },
            traits::Trades,
        },
    };
    use lobo_primitives::{CompressedPrice, Notional, Price64, Price128, uuid::Uuid};
    use lobo_storage::{
        MutatingFills, OrderStateError, SimulatedFills,
        bidask::{SidedOrderStore, traits::SidedPrice},
        policies::UpdateHiddenQuantity,
        price_level::{DeepPriceLevel, IntrusivePriceLevel, PriceLevelContract},
        price_sorting::{BTreeMapPriceSorting, PriceSortingPolicy, SortedVectorPriceSorting},
    };

    use super::{Book, Command, CommandError, CommandResult};

    fn p(raw: u32) -> CompressedPrice {
        CompressedPrice::from(raw)
    }

    fn new_book()
    -> Book<DeepPriceLevel<CompressedPrice, UpdateHiddenQuantity>, BTreeMapPriceSorting> {
        Book::default()
    }

    fn assert_price_representation_matches<L, Sort>()
    where
        L: PriceLevelContract,
        Sort: PriceSortingPolicy,
    {
        let mut book: Book<L, Sort> = Book::default();
        let maker_id = Uuid::new_v4();
        let maker = LimitOrder::<L::Price>::new(
            Some(L::Price::from(101_u32)),
            5,
            Uuid::new_v4(),
            Side::Sell,
        )
        .with_uuid(maker_id);

        let added = book
            .submit::<LimitOrderData, LimitOrderData, MutatingFills>(Command::Add { order: maker })
            .unwrap_or_else(|_| panic!("typed maker should be accepted"));
        assert!(matches!(added, CommandResult::Added { order_id } if order_id == maker_id));

        let taker = MarketOrder::<L::Price>::new(3, Uuid::new_v4(), Side::Buy);
        let filled = book
            .submit::<MarketOrderData, LimitOrderData, MutatingFills>(Command::Fill {
                order: taker,
                execution: MutatingFills,
                reports: Reports {
                    include_fills: true,
                    ..Reports::default()
                },
            })
            .unwrap_or_else(|_| panic!("typed taker should execute"));
        let CommandResult::Filled { report, .. } = filled else {
            panic!("typed fill returned wrong result")
        };
        let fill = &report.unwrap().fills.unwrap()[0];
        assert_eq!(fill.price, L::Price::from(101_u32));
        assert_eq!(fill.fill_quantity, 3);
    }

    #[test]
    fn book_is_statically_generic_over_all_builtin_price_representations() {
        macro_rules! assert_both_sorting_backends {
            ($level:ty) => {
                assert_price_representation_matches::<$level, BTreeMapPriceSorting>();
                assert_price_representation_matches::<$level, SortedVectorPriceSorting>();
            };
        }

        assert_both_sorting_backends!(DeepPriceLevel<CompressedPrice,UpdateHiddenQuantity>);
        assert_both_sorting_backends!(DeepPriceLevel<Price64,UpdateHiddenQuantity>);
        assert_both_sorting_backends!(DeepPriceLevel<Price128,UpdateHiddenQuantity>);
        assert_both_sorting_backends!(IntrusivePriceLevel<CompressedPrice,UpdateHiddenQuantity>);
        assert_both_sorting_backends!(IntrusivePriceLevel<Price64,UpdateHiddenQuantity>);
        assert_both_sorting_backends!(IntrusivePriceLevel<Price128,UpdateHiddenQuantity>);
    }

    fn add_limit(
        book: &mut Book<
            DeepPriceLevel<CompressedPrice, UpdateHiddenQuantity>,
            BTreeMapPriceSorting,
        >,
        order: LimitOrder<CompressedPrice>,
    ) -> Uuid {
        let expected_order_id = order.uuid();

        match book.submit::<LimitOrderData, LimitOrderData, MutatingFills>(Command::Add { order }) {
            Ok(CommandResult::Added { order_id }) => {
                assert_eq!(order_id, expected_order_id);
                order_id
            }
            Ok(_) => panic!("add returned the wrong command result"),
            Err(_) => panic!("add unexpectedly failed"),
        }
    }

    fn fill_limit(
        book: &mut Book<
            DeepPriceLevel<CompressedPrice, UpdateHiddenQuantity>,
            BTreeMapPriceSorting,
        >,
        order: LimitOrder<CompressedPrice>,
        execution: MutatingFills,
    ) -> Option<Uuid> {
        match book.submit::<LimitOrderData, LimitOrderData, MutatingFills>(Command::Fill {
            order,
            execution,
            reports: Reports::default(),
        }) {
            Ok(CommandResult::Filled {
                remaining_order_id, ..
            }) => remaining_order_id,
            Ok(_) => panic!("limit fill returned the wrong command result"),
            Err(_) => panic!("limit fill unexpectedly failed"),
        }
    }

    fn fill_market(
        book: &mut Book<
            DeepPriceLevel<CompressedPrice, UpdateHiddenQuantity>,
            BTreeMapPriceSorting,
        >,
        order: MarketOrder<CompressedPrice>,
        execution: MutatingFills,
    ) -> Option<Uuid> {
        match book.submit::<MarketOrderData, LimitOrderData, MutatingFills>(Command::Fill {
            order,
            execution,
            reports: Reports::default(),
        }) {
            Ok(CommandResult::Filled {
                remaining_order_id, ..
            }) => remaining_order_id,
            Ok(_) => panic!("market fill returned the wrong command result"),
            Err(_) => panic!("market fill unexpectedly failed"),
        }
    }

    fn cancel(
        book: &mut Book<
            DeepPriceLevel<CompressedPrice, UpdateHiddenQuantity>,
            BTreeMapPriceSorting,
        >,
        order_id: Uuid,
    ) -> Result<CommandResult<CompressedPrice>, CommandError> {
        book.submit::<LimitOrderData, LimitOrderData, MutatingFills>(Command::Cancel { order_id })
    }

    fn expect_canceled(
        book: &mut Book<
            DeepPriceLevel<CompressedPrice, UpdateHiddenQuantity>,
            BTreeMapPriceSorting,
        >,
        expected_order_id: Uuid,
    ) {
        match cancel(book, expected_order_id) {
            Ok(CommandResult::Canceled { order_id }) => {
                assert_eq!(order_id, expected_order_id);
            }
            Ok(_) => panic!("cancel returned the wrong command result"),
            Err(_) => panic!("cancel unexpectedly failed"),
        }
    }

    fn expect_order_missing(
        book: &mut Book<
            DeepPriceLevel<CompressedPrice, UpdateHiddenQuantity>,
            BTreeMapPriceSorting,
        >,
        order_id: Uuid,
    ) {
        assert!(matches!(
            cancel(book, order_id),
            Err(CommandError::OrderStateError(
                OrderStateError::OrderDoesNotExists
            ))
        ));
    }

    fn cancel_all(
        book: &mut Book<
            DeepPriceLevel<CompressedPrice, UpdateHiddenQuantity>,
            BTreeMapPriceSorting,
        >,
        user_id: Uuid,
    ) {
        match book.submit::<LimitOrderData, LimitOrderData, MutatingFills>(
            Command::CancelAllForUser { user_id },
        ) {
            Ok(CommandResult::CanceledAllForUser {
                user_id: canceled_user_id,
            }) => assert_eq!(canceled_user_id, user_id),
            Ok(_) => panic!("cancel-all returned the wrong command result"),
            Err(_) => panic!("cancel-all unexpectedly failed"),
        }
    }

    fn indexed_order_count(
        book: &Book<DeepPriceLevel<CompressedPrice, UpdateHiddenQuantity>, BTreeMapPriceSorting>,
    ) -> usize {
        book.order_storage
            .user_to_orders_map
            .values()
            .map(|orders| orders.len())
            .sum()
    }

    #[derive(Debug, PartialEq, Eq)]
    struct SideSnapshot {
        len: usize,
        visible_quantity: u64,
        hidden_quantity: u64,
        levels: Vec<(CompressedPrice, u64, u64, usize)>,
    }

    #[derive(Debug, PartialEq, Eq)]
    struct BookSnapshot {
        bids: SideSnapshot,
        asks: SideSnapshot,
        live_order_ids: Vec<Uuid>,
        indexed_orders_by_user: Vec<(Uuid, usize)>,
    }

    #[derive(Debug, PartialEq)]
    struct FillSnapshot {
        remaining_order_id: Option<Uuid>,
        fills: Vec<(Uuid, Uuid, u64, bool, Uuid, CompressedPrice)>,
        summary: Option<Summary<CompressedPrice>>,
        market_impact: Option<MarketImpact<CompressedPrice>>,
    }

    #[derive(Debug, PartialEq)]
    struct ParityTrace {
        states: Vec<BookSnapshot>,
        fills: Vec<FillSnapshot>,
        user_liquidity: Vec<(Uuid, usize, u64, u64, Notional)>,
    }

    fn side_snapshot<S, L, P>(store: &SidedOrderStore<S, L, P>) -> SideSnapshot
    where
        S: SidedPrice<CompressedPrice>,
        L: PriceLevelContract<Price = CompressedPrice>,
        P: PriceSortingPolicy,
    {
        SideSnapshot {
            len: store.len(),
            visible_quantity: store.visible_quantity,
            hidden_quantity: store.hidden_quantity,
            levels: store
                .price_levels()
                .map(|(price, level)| {
                    (
                        *price,
                        level.visible_quantity(),
                        level.hidden_quantity(),
                        level.len(),
                    )
                })
                .collect(),
        }
    }

    fn book_snapshot<L: PriceLevelContract<Price = CompressedPrice>, P: PriceSortingPolicy>(
        book: &Book<L, P>,
    ) -> BookSnapshot {
        let mut live_order_ids: Vec<_> = book
            .order_storage
            .order_to_arena_map
            .keys()
            .copied()
            .collect();
        live_order_ids.sort_unstable();

        let mut indexed_orders_by_user: Vec<_> = book
            .order_storage
            .user_to_orders_map
            .iter()
            .map(|(user_id, orders)| (*user_id, orders.len()))
            .collect();
        indexed_orders_by_user.sort_unstable_by_key(|(user_id, _)| *user_id);

        BookSnapshot {
            bids: side_snapshot(&book.order_storage.bids),
            asks: side_snapshot(&book.order_storage.asks),
            live_order_ids,
            indexed_orders_by_user,
        }
    }

    fn fill_snapshot(result: Result<CommandResult<CompressedPrice>, CommandError>) -> FillSnapshot {
        let CommandResult::Filled {
            remaining_order_id,
            report,
        } = result.unwrap_or_else(|_| panic!("parity fill unexpectedly failed"))
        else {
            panic!("parity fill returned the wrong command result");
        };
        let report = report.expect("parity fill did not return its requested report");
        let fills = report
            .fills
            .unwrap_or_default()
            .into_iter()
            .map(|fill| {
                (
                    fill.maker_order_id,
                    fill.taker_order_id,
                    fill.fill_quantity,
                    fill.maker_depleted,
                    fill.maker_trader_uid,
                    fill.price,
                )
            })
            .collect();

        FillSnapshot {
            remaining_order_id,
            fills,
            summary: report.summary,
            market_impact: report.market_impact,
        }
    }

    fn add_parity_limit<L: PriceLevelContract<Price = CompressedPrice>, P: PriceSortingPolicy>(
        book: &mut Book<L, P>,
        order: LimitOrder<CompressedPrice>,
        order_id: Uuid,
    ) {
        let result = book
            .submit::<LimitOrderData, LimitOrderData, MutatingFills>(Command::Add {
                order: order.with_uuid(order_id),
            })
            .unwrap_or_else(|_| panic!("parity limit insertion failed"));
        assert!(matches!(result, CommandResult::Added { order_id: added } if added == order_id));
    }

    fn apply_parity_order_details(
        order: &mut RestingOrder<CompressedPrice>,
        details: OrderDetails<CompressedPrice>,
    ) {
        order.update_in_place(details);
    }

    fn parity_order_details(
        quantity: u64,
        price: Option<CompressedPrice>,
        uuid: Option<Uuid>,
        side: Option<Side>,
    ) -> OrderDetails<CompressedPrice> {
        OrderDetails {
            uuid,
            price,
            creation_time: None,
            quantity: Some(quantity),
            trader: None,
            side,
        }
    }

    fn run_book_parity_scenario<
        L: PriceLevelContract<Price = CompressedPrice>,
        P: PriceSortingPolicy,
    >() -> ParityTrace {
        let buyer_one = Uuid::from_u128(1);
        let buyer_two = Uuid::from_u128(2);
        let buyer_three = Uuid::from_u128(3);
        let seller_one = Uuid::from_u128(4);
        let seller_two = Uuid::from_u128(5);
        let bid_100 = Uuid::from_u128(101);
        let first_bid_101 = Uuid::from_u128(102);
        let second_bid_101 = Uuid::from_u128(103);
        let side_moved_bid_98 = Uuid::from_u128(104);
        let replaced_bid_99 = Uuid::from_u128(105);
        let replaced_bid_101 = Uuid::from_u128(106);
        let first_ask_102 = Uuid::from_u128(201);
        let second_ask_102 = Uuid::from_u128(202);
        let iceberg_103 = Uuid::from_u128(203);
        let simulated_taker = Uuid::from_u128(301);
        let buy_taker = Uuid::from_u128(302);
        let sell_taker = Uuid::from_u128(303);
        let resting_taker = Uuid::from_u128(304);
        let reports = Reports {
            include_fills: true,
            include_summary: true,
            include_market_impact: true,
        };
        let mut book = Book::<L, P>::default();
        let mut states = Vec::new();
        let mut fills = Vec::new();

        add_parity_limit(
            &mut book,
            LimitOrder::new(Some(100), 3, buyer_one, Side::Buy),
            bid_100,
        );
        add_parity_limit(
            &mut book,
            LimitOrder::new(Some(101), 2, buyer_one, Side::Buy),
            first_bid_101,
        );
        add_parity_limit(
            &mut book,
            LimitOrder::new(Some(101), 4, buyer_two, Side::Buy),
            second_bid_101,
        );
        add_parity_limit(
            &mut book,
            LimitOrder::new(Some(98), 1, buyer_two, Side::Buy),
            side_moved_bid_98,
        );
        add_parity_limit(
            &mut book,
            LimitOrder::new(Some(102), 2, seller_one, Side::Sell),
            first_ask_102,
        );
        add_parity_limit(
            &mut book,
            LimitOrder::new(Some(102), 3, seller_two, Side::Sell),
            second_ask_102,
        );
        let result = book
            .submit::<IcebergOrderData, IcebergOrderData, MutatingFills>(Command::Add {
                order: IcebergOrder::new(Some(103), seller_one, Side::Sell, 5, 2)
                    .with_uuid(iceberg_103),
            })
            .unwrap_or_else(|_| panic!("parity iceberg insertion failed"));
        assert!(matches!(result, CommandResult::Added { order_id } if order_id == iceberg_103));
        states.push(book_snapshot(&book));

        book.order_storage
            .modify_order_in_place(
                bid_100,
                parity_order_details(4, None, None, None),
                apply_parity_order_details,
                &mut |_| {},
            )
            .unwrap_or_else(|_| panic!("parity quantity modification failed"));
        states.push(book_snapshot(&book));

        let replaced = book
            .order_storage
            .replace_in_place(
                bid_100,
                parity_order_details(4, Some(p(99)), Some(replaced_bid_99), None),
                apply_parity_order_details,
                &mut |_| {},
            )
            .unwrap_or_else(|_| panic!("parity price replacement failed"));
        assert_eq!(replaced, replaced_bid_99);
        states.push(book_snapshot(&book));

        let replaced = book
            .order_storage
            .replace_in_place(
                first_bid_101,
                parity_order_details(2, Some(p(101)), Some(replaced_bid_101), None),
                apply_parity_order_details,
                &mut |_| {},
            )
            .unwrap_or_else(|_| panic!("parity FIFO replacement failed"));
        assert_eq!(replaced, replaced_bid_101);
        states.push(book_snapshot(&book));

        book.order_storage
            .replace_in_place(
                side_moved_bid_98,
                parity_order_details(1, Some(p(105)), None, Some(Side::Sell)),
                apply_parity_order_details,
                &mut |_| {},
            )
            .unwrap_or_else(|_| panic!("parity side replacement failed"));
        states.push(book_snapshot(&book));

        fills.push(fill_snapshot(
            book.submit::<MarketOrderData, LimitOrderData, SimulatedFills>(Command::Fill {
                order: MarketOrder::new(8, buyer_three, Side::Buy).with_uuid(simulated_taker),
                execution: SimulatedFills,
                reports,
            }),
        ));
        states.push(book_snapshot(&book));

        let canceled = book
            .submit::<LimitOrderData, LimitOrderData, MutatingFills>(Command::Cancel {
                order_id: second_ask_102,
            })
            .unwrap_or_else(|_| panic!("parity cancellation failed"));
        assert!(
            matches!(canceled, CommandResult::Canceled { order_id } if order_id == second_ask_102)
        );
        states.push(book_snapshot(&book));

        fills.push(fill_snapshot(
            book.submit::<MarketOrderData, LimitOrderData, MutatingFills>(Command::Fill {
                order: MarketOrder::new(5, buyer_three, Side::Buy).with_uuid(buy_taker),
                execution: MutatingFills,
                reports,
            }),
        ));
        states.push(book_snapshot(&book));

        fills.push(fill_snapshot(
            book.submit::<LimitOrderData, LimitOrderData, MutatingFills>(Command::Fill {
                order: LimitOrder::new(Some(100), 5, seller_two, Side::Sell).with_uuid(sell_taker),
                execution: MutatingFills,
                reports,
            }),
        ));
        states.push(book_snapshot(&book));

        let canceled = book
            .submit::<LimitOrderData, LimitOrderData, MutatingFills>(Command::Cancel {
                order_id: replaced_bid_101,
            })
            .unwrap_or_else(|_| panic!("parity partial-maker cancellation failed"));
        assert!(
            matches!(canceled, CommandResult::Canceled { order_id } if order_id == replaced_bid_101)
        );
        states.push(book_snapshot(&book));

        fills.push(fill_snapshot(
            book.submit::<LimitOrderData, LimitOrderData, MutatingFills>(Command::Fill {
                order: LimitOrder::new(Some(103), 7, buyer_three, Side::Buy)
                    .with_uuid(resting_taker),
                execution: MutatingFills,
                reports,
            }),
        ));
        states.push(book_snapshot(&book));

        for user_id in [buyer_one, buyer_two, buyer_three] {
            let canceled = book
                .submit::<LimitOrderData, LimitOrderData, MutatingFills>(
                    Command::CancelAllForUser { user_id },
                )
                .unwrap_or_else(|_| panic!("parity cancel-all failed"));
            assert!(
                matches!(canceled, CommandResult::CanceledAllForUser { user_id: canceled } if canceled == user_id)
            );
            states.push(book_snapshot(&book));
        }

        let user_liquidity = [buyer_one, buyer_two, buyer_three, seller_one, seller_two]
            .into_iter()
            .map(|user_id| {
                let liquidity = book.order_storage.user_outstanding_liquidity(user_id);
                (
                    user_id,
                    liquidity.order_count(),
                    liquidity.visible_quantity(),
                    liquidity.hidden_quantity(),
                    liquidity.value(),
                )
            })
            .collect();
        states.push(book_snapshot(&book));

        ParityTrace {
            states,
            fills,
            user_liquidity,
        }
    }

    #[test]
    fn deep_and_intrusive_books_have_command_and_state_parity() {
        let deep = run_book_parity_scenario::<
            DeepPriceLevel<CompressedPrice, UpdateHiddenQuantity>,
            SortedVectorPriceSorting,
        >();
        let intrusive = run_book_parity_scenario::<
            IntrusivePriceLevel<CompressedPrice, UpdateHiddenQuantity>,
            SortedVectorPriceSorting,
        >();

        assert_eq!(intrusive, deep);
    }

    #[test]
    fn sorted_vector_and_btree_map_price_sorting_have_command_and_state_parity() {
        let sorted = run_book_parity_scenario::<
            IntrusivePriceLevel<CompressedPrice, UpdateHiddenQuantity>,
            SortedVectorPriceSorting,
        >();
        let btree = run_book_parity_scenario::<
            IntrusivePriceLevel<CompressedPrice, UpdateHiddenQuantity>,
            BTreeMapPriceSorting,
        >();

        assert_eq!(btree, sorted);
    }

    #[test]
    fn price_sorting_policy_is_selected_when_the_book_is_instantiated() {
        let book =
            Book::<DeepPriceLevel<Price64, UpdateHiddenQuantity>, BTreeMapPriceSorting>::default();

        assert!(book.order_storage.bids.is_empty());
        assert!(book.order_storage.asks.is_empty());
    }

    #[test]
    fn add_and_cancel_report_order_ids_and_duplicate_or_missing_orders() {
        let mut book = new_book();
        let trader = Uuid::new_v4();
        let order_id = Uuid::new_v4();

        add_limit(
            &mut book,
            LimitOrder::new(Some(100), 7, trader, Side::Buy).with_uuid(order_id),
        );

        assert!(
            book.order_storage
                .order_to_arena_map
                .contains_key(&order_id)
        );

        let duplicate = LimitOrder::new(Some(100), 7, trader, Side::Buy).with_uuid(order_id);
        assert!(matches!(
            book.submit::<LimitOrderData, LimitOrderData, MutatingFills>(Command::Add {
                order: duplicate
            }),
            Err(CommandError::OrderStateError(
                OrderStateError::OrderAlreadyExists
            ))
        ));

        expect_canceled(&mut book, order_id);
        assert!(
            !book
                .order_storage
                .order_to_arena_map
                .contains_key(&order_id)
        );
        expect_order_missing(&mut book, order_id);
        expect_order_missing(&mut book, Uuid::new_v4());
    }

    #[test]
    fn unmatched_limit_rests_and_returns_its_id() {
        let trader = Uuid::new_v4();
        let mut book = new_book();
        let limit = LimitOrder::new(Some(100), 5, trader, Side::Buy);
        let limit_id = limit.uuid();

        assert_eq!(fill_limit(&mut book, limit, MutatingFills), Some(limit_id));
        expect_canceled(&mut book, limit_id);
    }

    #[test]
    fn unmatched_market_expires_without_resting() {
        let trader = Uuid::new_v4();
        let mut book = new_book();
        let market = MarketOrder::new(5, trader, Side::Buy);
        let market_id = market.uuid();

        assert_eq!(fill_market(&mut book, market, MutatingFills), None);
        expect_order_missing(&mut book, market_id);
        assert!(book.order_storage.order_to_arena_map.is_empty());
    }

    #[test]
    fn exact_fill_has_no_remainder() {
        let maker = Uuid::new_v4();
        let taker = Uuid::new_v4();
        let mut book = new_book();
        let maker_id = add_limit(&mut book, LimitOrder::new(Some(100), 5, maker, Side::Sell));
        let exact_taker = LimitOrder::new(Some(100), 5, taker, Side::Buy);
        let exact_taker_id = exact_taker.uuid();

        assert_eq!(fill_limit(&mut book, exact_taker, MutatingFills), None);
        expect_order_missing(&mut book, maker_id);
        expect_order_missing(&mut book, exact_taker_id);
    }

    #[test]
    fn simulated_fill_reports_execution_without_mutating_the_book() {
        let maker = Uuid::new_v4();
        let taker = Uuid::new_v4();
        let mut book = new_book();
        let maker_id = add_limit(&mut book, LimitOrder::new(Some(100), 5, maker, Side::Sell));
        let taker_order = LimitOrder::new(Some(100), 7, taker, Side::Buy);
        let taker_id = taker_order.uuid();

        let result = book
            .submit::<LimitOrderData, LimitOrderData, SimulatedFills>(Command::Fill {
                order: taker_order,
                execution: SimulatedFills,
                reports: Reports {
                    include_summary: true,
                    ..Reports::default()
                },
            })
            .ok()
            .unwrap();

        match result {
            CommandResult::Filled {
                remaining_order_id,
                report,
            } => {
                assert_eq!(remaining_order_id, Some(taker_id));
                let summary = report.unwrap().summary.unwrap();
                assert_eq!(summary.filled_quantity, 5);
                assert_eq!(summary.realized_price, 100);
            }
            _ => panic!("simulated fill returned the wrong command result"),
        }

        assert_eq!(book.order_storage.asks.len(), 1);
        assert_eq!(book.order_storage.asks.visible_quantity, 5);
        assert!(book.order_storage.bids.is_empty());
        assert_eq!(book.order_storage.order_to_arena_map.len(), 1);
        assert!(
            book.order_storage
                .order_to_arena_map
                .contains_key(&maker_id)
        );
        assert!(
            !book
                .order_storage
                .order_to_arena_map
                .contains_key(&taker_id)
        );
    }

    fn filled_report(
        result: Result<CommandResult<CompressedPrice>, CommandError>,
    ) -> (Option<Uuid>, Report<CompressedPrice>) {
        match result {
            Ok(CommandResult::Filled {
                remaining_order_id,
                report: Some(report),
            }) => (remaining_order_id, report),
            Ok(CommandResult::Filled { report: None, .. }) => {
                panic!("requested report was not returned")
            }
            Ok(_) => panic!("fill returned the wrong command result"),
            Err(_) => panic!("fill unexpectedly failed"),
        }
    }

    fn simulation_and_mutation_reports(
        reports: Reports,
    ) -> (Report<CompressedPrice>, Report<CompressedPrice>) {
        let maker = Uuid::new_v4();
        let taker = Uuid::new_v4();
        let taker_order_id = Uuid::new_v4();
        let mut book = new_book();

        add_limit(&mut book, LimitOrder::new(Some(100), 2, maker, Side::Sell));
        add_limit(&mut book, LimitOrder::new(Some(101), 3, maker, Side::Sell));
        book.submit::<IcebergOrderData, IcebergOrderData, MutatingFills>(Command::Add {
            order: IcebergOrder::new(Some(102), maker, Side::Sell, 4, 2),
        })
        .ok()
        .expect("iceberg fixture insertion failed");

        let simulated = filled_report(
            book.submit::<MarketOrderData, LimitOrderData, SimulatedFills>(Command::Fill {
                order: MarketOrder::new(8, taker, Side::Buy).with_uuid(taker_order_id),
                execution: SimulatedFills,
                reports,
            }),
        );
        let mutated = filled_report(
            book.submit::<MarketOrderData, LimitOrderData, MutatingFills>(Command::Fill {
                order: MarketOrder::new(8, taker, Side::Buy).with_uuid(taker_order_id),
                execution: MutatingFills,
                reports,
            }),
        );

        assert_eq!(simulated.0, mutated.0);
        (simulated.1, mutated.1)
    }

    #[test]
    fn simulated_fills_match_mutating_fills() {
        let reports = Reports {
            include_fills: true,
            ..Reports::default()
        };
        let (simulated, mutated) = simulation_and_mutation_reports(reports);
        let simulated_fills = simulated.fills.expect("fills were requested");
        let mutated_fills = mutated.fills.expect("fills were requested");

        assert_eq!(simulated_fills.len(), mutated_fills.len());
        for (simulated, mutated) in simulated_fills.iter().zip(&mutated_fills) {
            assert_eq!(simulated.maker_order_id, mutated.maker_order_id);
            assert_eq!(simulated.taker_order_id, mutated.taker_order_id);
            assert_eq!(simulated.fill_quantity, mutated.fill_quantity);
            assert_eq!(simulated.maker_depleted, mutated.maker_depleted);
            assert_eq!(simulated.maker_trader_uid, mutated.maker_trader_uid);
            assert_eq!(simulated.price, mutated.price);
        }

        assert_eq!(
            simulated_fills
                .iter()
                .map(|fill| (fill.fill_quantity, fill.price))
                .collect::<Vec<_>>(),
            [(2, p(100)), (3, p(101)), (2, p(102)), (1, p(102))]
        );
    }

    #[test]
    fn simulated_summary_matches_mutating_summary() {
        let reports = Reports {
            include_summary: true,
            ..Reports::default()
        };
        let (simulated, mutated) = simulation_and_mutation_reports(reports);
        let simulated = simulated.summary.expect("summary was requested");
        let mutated = mutated.summary.expect("summary was requested");

        assert_eq!(simulated, mutated);
        assert_eq!(simulated.filled_quantity, 8);
        assert_eq!(simulated.realized_price, 101);
        assert_eq!(simulated.side, Side::Buy);
    }

    #[test]
    fn simulated_market_impact_matches_mutating_market_impact() {
        let reports = Reports {
            include_market_impact: true,
            ..Reports::default()
        };
        let (simulated, mutated) = simulation_and_mutation_reports(reports);
        let simulated = simulated
            .market_impact
            .expect("market impact was requested");
        let mutated = mutated.market_impact.expect("market impact was requested");

        assert_eq!(simulated, mutated);
        assert_eq!(simulated.avg_price, 809.0 / 8.0);
        assert_eq!(simulated.worst_price, 102);
        assert_eq!(simulated.slippage, 2);
        assert_eq!(simulated.slippage_bps, 200.0);
        assert_eq!(simulated.levels_consumed, 3);
        assert_eq!(simulated.total_quantity_available, 11);
    }

    #[test]
    fn partially_filled_limit_taker_rests_and_returns_its_id() {
        let maker = Uuid::new_v4();
        let taker = Uuid::new_v4();
        let mut book = new_book();
        let partial_maker_id =
            add_limit(&mut book, LimitOrder::new(Some(100), 2, maker, Side::Sell));
        let partial_taker = LimitOrder::new(Some(100), 5, taker, Side::Buy);
        let partial_taker_id = partial_taker.uuid();

        assert_eq!(
            fill_limit(&mut book, partial_taker, MutatingFills),
            Some(partial_taker_id)
        );
        expect_order_missing(&mut book, partial_maker_id);
        expect_canceled(&mut book, partial_taker_id);
    }

    #[test]
    fn fill_result_reports_quantity_and_weighted_average_price_across_levels() {
        let maker = Uuid::new_v4();
        let taker = Uuid::new_v4();
        let mut book = new_book();

        add_limit(&mut book, LimitOrder::new(Some(100), 1, maker, Side::Sell));
        add_limit(&mut book, LimitOrder::new(Some(102), 3, maker, Side::Sell));

        let result =
            match book.submit::<LimitOrderData, LimitOrderData, MutatingFills>(Command::Fill {
                order: LimitOrder::new(Some(102), 4, taker, Side::Buy),
                execution: MutatingFills,
                reports: Reports {
                    include_summary: true,
                    ..Reports::default()
                },
            }) {
                Ok(result) => result,
                Err(_) => panic!("crossing order should be accepted"),
            };

        match result {
            CommandResult::Filled {
                remaining_order_id,
                report,
            } => {
                assert_eq!(remaining_order_id, None);
                let summary = report.unwrap().summary.unwrap();
                assert_eq!(summary.trader_id, taker);
                assert_eq!(summary.filled_quantity, 4);
                assert_eq!(summary.realized_price, 101);
            }
            _ => panic!("fill returned the wrong command result"),
        }
    }

    #[test]
    fn matching_uses_best_price_on_both_sides() {
        let buyer = Uuid::new_v4();
        let seller = Uuid::new_v4();

        let mut bid_book = new_book();
        let bid_99 = add_limit(
            &mut bid_book,
            LimitOrder::new(Some(99), 1, buyer, Side::Buy),
        );
        let bid_101 = add_limit(
            &mut bid_book,
            LimitOrder::new(Some(101), 1, buyer, Side::Buy),
        );
        let bid_100 = add_limit(
            &mut bid_book,
            LimitOrder::new(Some(100), 1, buyer, Side::Buy),
        );

        assert_eq!(
            fill_limit(
                &mut bid_book,
                LimitOrder::new(Some(99), 1, seller, Side::Sell),
                MutatingFills,
            ),
            None
        );
        expect_order_missing(&mut bid_book, bid_101);
        expect_canceled(&mut bid_book, bid_100);
        expect_canceled(&mut bid_book, bid_99);

        let mut ask_book = new_book();
        let ask_102 = add_limit(
            &mut ask_book,
            LimitOrder::new(Some(102), 1, seller, Side::Sell),
        );
        let ask_100 = add_limit(
            &mut ask_book,
            LimitOrder::new(Some(100), 1, seller, Side::Sell),
        );
        let ask_101 = add_limit(
            &mut ask_book,
            LimitOrder::new(Some(101), 1, seller, Side::Sell),
        );

        assert_eq!(
            fill_limit(
                &mut ask_book,
                LimitOrder::new(Some(102), 1, buyer, Side::Buy),
                MutatingFills,
            ),
            None
        );
        expect_order_missing(&mut ask_book, ask_100);
        expect_canceled(&mut ask_book, ask_101);
        expect_canceled(&mut ask_book, ask_102);
    }

    #[test]
    fn matching_uses_submission_time_within_a_price_level() {
        let buyer = Uuid::new_v4();
        let seller = Uuid::new_v4();

        let mut bid_book = new_book();
        let first_bid = add_limit(
            &mut bid_book,
            LimitOrder::new(Some(100), 1, buyer, Side::Buy),
        );
        let second_bid = add_limit(
            &mut bid_book,
            LimitOrder::new(Some(100), 1, buyer, Side::Buy),
        );
        let third_bid = add_limit(
            &mut bid_book,
            LimitOrder::new(Some(100), 1, buyer, Side::Buy),
        );

        assert_eq!(
            fill_limit(
                &mut bid_book,
                LimitOrder::new(Some(100), 1, seller, Side::Sell),
                MutatingFills,
            ),
            None
        );
        expect_order_missing(&mut bid_book, first_bid);
        expect_canceled(&mut bid_book, second_bid);
        expect_canceled(&mut bid_book, third_bid);

        let mut ask_book = new_book();
        let first_ask = add_limit(
            &mut ask_book,
            LimitOrder::new(Some(100), 1, seller, Side::Sell),
        );
        let second_ask = add_limit(
            &mut ask_book,
            LimitOrder::new(Some(100), 1, seller, Side::Sell),
        );
        let third_ask = add_limit(
            &mut ask_book,
            LimitOrder::new(Some(100), 1, seller, Side::Sell),
        );

        assert_eq!(
            fill_limit(
                &mut ask_book,
                LimitOrder::new(Some(100), 1, buyer, Side::Buy),
                MutatingFills,
            ),
            None
        );
        expect_order_missing(&mut ask_book, first_ask);
        expect_canceled(&mut ask_book, second_ask);
        expect_canceled(&mut ask_book, third_ask);
    }

    #[test]
    fn cancel_all_removes_only_the_selected_users_orders() {
        let canceled_user = Uuid::new_v4();
        let active_user = Uuid::new_v4();
        let seller = Uuid::new_v4();
        let mut book = new_book();

        let canceled_101 = add_limit(
            &mut book,
            LimitOrder::new(Some(101), 1, canceled_user, Side::Buy),
        );
        let canceled_100 = add_limit(
            &mut book,
            LimitOrder::new(Some(100), 1, canceled_user, Side::Buy),
        );
        let active_99 = add_limit(
            &mut book,
            LimitOrder::new(Some(99), 1, active_user, Side::Buy),
        );

        cancel_all(&mut book, canceled_user);

        expect_order_missing(&mut book, canceled_101);
        expect_order_missing(&mut book, canceled_100);
        assert!(
            book.order_storage
                .order_to_arena_map
                .contains_key(&active_99)
        );

        assert_eq!(
            fill_limit(
                &mut book,
                LimitOrder::new(Some(99), 1, seller, Side::Sell),
                MutatingFills,
            ),
            None
        );
        expect_order_missing(&mut book, active_99);
        assert_eq!(
            book.order_storage.order_to_arena_map.len(),
            0,
            "fully depleted makers must be removed from the order index"
        );
    }

    #[test]
    fn matches_ten_thousand_one_for_one_trades_through_the_command_api() {
        const TRADE_COUNT: usize = 10_000;

        let buyer = Uuid::new_v4();
        let seller = Uuid::new_v4();
        let mut book = new_book();

        for _ in 0..TRADE_COUNT {
            add_limit(&mut book, LimitOrder::new(Some(100), 1, buyer, Side::Buy));
        }

        assert_eq!(book.order_storage.order_to_arena_map.len(), TRADE_COUNT);
        assert_eq!(indexed_order_count(&book), TRADE_COUNT);

        for _ in 0..TRADE_COUNT {
            assert_eq!(
                fill_limit(
                    &mut book,
                    LimitOrder::new(Some(100), 1, seller, Side::Sell),
                    MutatingFills,
                ),
                None
            );
        }

        assert!(book.order_storage.bids.is_empty());
        assert!(book.order_storage.asks.is_empty());
        assert_eq!(
            book.order_storage.order_to_arena_map.len(),
            0,
            "fully depleted makers must be removed from the order index"
        );
        assert_eq!(indexed_order_count(&book), TRADE_COUNT);
        assert_eq!(
            book.order_storage
                .user_outstanding_liquidity(buyer)
                .order_count(),
            0
        );
        assert_eq!(indexed_order_count(&book), 0);
    }

    #[test]
    fn one_order_sweeps_ten_thousand_makers_across_price_levels() {
        const PRICE_LEVELS: usize = 20;
        const ORDERS_PER_LEVEL: usize = 500;
        const TRADE_COUNT: usize = PRICE_LEVELS * ORDERS_PER_LEVEL;

        let buyer = Uuid::new_v4();
        let seller = Uuid::new_v4();
        let mut book = new_book();

        for level in 0..PRICE_LEVELS {
            let price = p(100 + u32::try_from(level).unwrap());

            for _ in 0..ORDERS_PER_LEVEL {
                add_limit(
                    &mut book,
                    LimitOrder::new(Some(price), 1, seller, Side::Sell),
                );
            }
        }

        assert_eq!(book.order_storage.asks.len(), TRADE_COUNT);
        assert_eq!(book.order_storage.asks.price_level_count(), PRICE_LEVELS);
        assert_eq!(book.order_storage.order_to_arena_map.len(), TRADE_COUNT);

        assert_eq!(
            fill_limit(
                &mut book,
                LimitOrder::new(
                    Some(p(100 + u32::try_from(PRICE_LEVELS - 1).unwrap())),
                    TRADE_COUNT as u64,
                    buyer,
                    Side::Buy,
                ),
                MutatingFills,
            ),
            None
        );

        assert!(book.order_storage.asks.is_empty());
        assert!(book.order_storage.bids.is_empty());
        assert_eq!(
            book.order_storage.order_to_arena_map.len(),
            0,
            "fully depleted makers must be removed from the order index"
        );
        assert_eq!(indexed_order_count(&book), TRADE_COUNT);
        assert_eq!(
            book.order_storage
                .user_outstanding_liquidity(seller)
                .order_count(),
            0
        );
        assert_eq!(indexed_order_count(&book), 0);
    }
}
