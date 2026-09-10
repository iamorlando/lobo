use super::adapter::{PythonReplayResult, ReplayOptions, ReplayOutput};
use crate::custom::{definition::binary::FileFormat, source::CustomReplaySource};
use lobo_models::BookPolicy;
use pyo3::prelude::*;
use std::collections::HashMap;

macro_rules! policy {
    ($module:ident,$users:ty,$hidden:ty)=>{
        mod $module {
            use super::*;
            use lobo_books::price_time_priority::Book;
            use lobo_context::python::ConnectedPublisher;
            use lobo_events::{BookPublisherFactory,NullPublisher};
            use lobo_primitives::CompressedPrice;
            use lobo_storage::{price_level::IntrusivePriceLevel,price_sorting::SortedVectorPriceSorting};
            use pyo3::types::PyDict;
            type Level=IntrusivePriceLevel<CompressedPrice,$hidden>;
            type ReplayBook<P=NullPublisher>=Book<Level,SortedVectorPriceSorting,$users,$hidden,P>;
            use lobo_books::price_time_priority::PyBook;
            struct Output<P>{books:HashMap<String,ReplayBook<P>>,stats:serde_json::Value}
            impl<P:BookPublisherFactory<CompressedPrice>+Send+'static> ReplayOutput for Output<P>
            where PyBook:From<ReplayBook<P>> {
                fn into_python(self:Box<Self>,py:Python<'_>)->PyResult<PythonReplayResult>{
                    let dictionary=PyDict::new(py);let mut handles=Vec::with_capacity(self.books.len());
                    for (symbol,book) in self.books{let book=Py::new(py,PyBook::from(book))?;dictionary.set_item(symbol,&book)?;handles.push(book);}
                    Ok(PythonReplayResult{value:dictionary.into_any().unbind(),stats:self.stats,disconnect:Box::new(move|py|{for book in handles{book.try_borrow_mut(py)?.book.disconnect();}Ok(())})})
                }
            }
            pub(super) fn replay(mut options:ReplayOptions,sources:Vec<CustomReplaySource<FileFormat>>)->Result<Box<dyn ReplayOutput>,String>{
                match options.publisher.take(){Some(publisher)=>run(options,sources,publisher),None=>run(options,sources,NullPublisher)}
            }
            fn run<P:BookPublisherFactory<CompressedPrice>+Send+'static>(options:ReplayOptions,sources:Vec<CustomReplaySource<FileFormat>>,publisher:P)->Result<Box<dyn ReplayOutput>,String>
            where PyBook:From<ReplayBook<P>> {
                let mut context=crate::ReplayContext::<Level,SortedVectorPriceSorting,$users,$hidden,_>::with_sinks(options.cutoff,ConnectedPublisher(publisher)).map_err(|e|e.to_string())?;
                let sources=sources.iter().collect::<Vec<_>>();
                let stats=if options.collect_stats{serde_json::to_value(context.replay_from_sources_with_stats(&sources)?).map_err(|e|e.to_string())?}else{
                    #[cfg(feature="concurrent")]
                    if options.concurrent{context.parallel_replay_from_sources(&sources,12)?;}else{context.replay_from_sources(&sources)?;}
                    #[cfg(not(feature="concurrent"))]
                    {let _=options.concurrent;context.replay_from_sources(&sources)?;}
                    serde_json::Value::Null
                };
                Ok(Box::new(Output{books:context.books,stats}))
            }
        }
    }
}
policy!(
    full,
    lobo_storage::UpdateUserMap,
    lobo_storage::policies::UpdateHiddenQuantity
);
policy!(
    no_user_map,
    lobo_storage::DoNotUpdateUserMap,
    lobo_storage::policies::UpdateHiddenQuantity
);
policy!(
    no_hidden_quantity,
    lobo_storage::UpdateUserMap,
    lobo_storage::policies::DoNotUpdateHiddenQuantity
);
policy!(
    no_updates,
    lobo_storage::DoNotUpdateUserMap,
    lobo_storage::policies::DoNotUpdateHiddenQuantity
);

pub(super) fn replay(
    options: ReplayOptions,
    sources: Vec<CustomReplaySource<FileFormat>>,
    policy: BookPolicy,
) -> Result<Box<dyn ReplayOutput>, String> {
    match policy {
        BookPolicy::Full => full::replay(options, sources),
        BookPolicy::NoUserMap => no_user_map::replay(options, sources),
        BookPolicy::NoHiddenQuantity => no_hidden_quantity::replay(options, sources),
        BookPolicy::NoUpdates => no_updates::replay(options, sources),
    }
}

/// Policy groups keep their concrete book types throughout replay.
pub(super) struct Combined(pub Vec<Box<dyn ReplayOutput>>);
impl ReplayOutput for Combined {
    fn into_python(self: Box<Self>, py: Python<'_>) -> PyResult<PythonReplayResult> {
        use pyo3::types::PyDict;
        let books = PyDict::new(py);
        let mut stats = serde_json::Map::new();
        let mut cleanup = Vec::new();
        for output in self.0 {
            let result = output.into_python(py)?;
            books.update(result.value.bind(py).cast::<PyDict>()?.as_mapping())?;
            if let Some(values) = result.stats.as_object() {
                for (key, value) in values {
                    if let Some(value) = value.as_u64() {
                        let previous = stats
                            .get(key)
                            .and_then(serde_json::Value::as_u64)
                            .unwrap_or(0);
                        stats.insert(
                            key.clone(),
                            if key == "source_messages" {
                                previous.max(value)
                            } else {
                                previous + value
                            }
                            .into(),
                        );
                    }
                }
            }
            cleanup.push(result.disconnect);
        }
        Ok(PythonReplayResult {
            value: books.into_any().unbind(),
            stats: if stats.is_empty() {
                serde_json::Value::Null
            } else {
                stats.into()
            },
            disconnect: Box::new(move |py| {
                for disconnect in cleanup {
                    disconnect(py)?;
                }
                Ok(())
            }),
        })
    }
}
