//! HTTP replay bytes, gzip decompression and decoding stay on the worker.
//! The same protocol consumes local files and online sources. Only bounded
//! decompressed chunks reach the protocol; no download-to-disk step is needed.
use super::{
    MarketDataAdapter,
    runtime::{ControlReceiver, advance},
};
use flate2::write::MultiGzDecoder;
use std::{
    future::Future,
    io::{self, Write},
    time::Duration,
};

struct Input<'a> {
    adapter: &'a mut dyn MarketDataAdapter,
    controls: &'a ControlReceiver,
    chunk_size: usize,
}
impl Write for Input<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        for chunk in bytes.chunks(self.chunk_size) {
            if !self
                .controls
                .ready(self.adapter)
                .map_err(io::Error::other)?
            {
                return Err(io::ErrorKind::ConnectionAborted.into());
            }
            if !self
                .controls
                .receive(self.adapter, chunk, false)
                .map_err(io::Error::other)?
            {
                return Err(io::ErrorKind::ConnectionAborted.into());
            }
            if !advance(self.adapter, self.controls).map_err(io::Error::other)? {
                return Err(io::ErrorKind::ConnectionAborted.into());
            }
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
enum Decoder<'a> {
    Plain(Input<'a>),
    Gzip(MultiGzDecoder<Input<'a>>),
}
impl<'a> Decoder<'a> {
    fn input(&mut self) -> &mut Input<'a> {
        match self {
            Self::Plain(input) => input,
            Self::Gzip(gzip) => gzip.get_mut(),
        }
    }
    fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        match self {
            Self::Plain(input) => input.write_all(bytes),
            Self::Gzip(gzip) => gzip.write_all(bytes),
        }
    }
    fn finish(&mut self) -> Result<(), String> {
        if let Self::Gzip(gzip) = self {
            gzip.try_finish().map_err(|e| e.to_string())?;
        }
        let input = self.input();
        input.controls.receive(input.adapter, &[], true)?;
        advance(input.adapter, input.controls)?;
        Ok(())
    }
}
/// Keep controls responsive during DNS, connection setup, and stalled reads.
async fn waiting<F: Future>(
    future: F,
    adapter: &mut dyn MarketDataAdapter,
    controls: &ControlReceiver,
) -> Option<F::Output> {
    tokio::pin!(future);
    loop {
        tokio::select! {
            output=&mut future=>return Some(output),
            _=tokio::time::sleep(Duration::from_millis(10))=>if !controls.poll(adapter) {return None;},
        }
    }
}
pub(super) fn run(
    adapter: &mut dyn MarketDataAdapter,
    controls: &ControlReceiver,
    url: String,
    chunk_size: usize,
) -> Result<(), String> {
    if chunk_size == 0 || chunk_size > 4 * 1024 * 1024 {
        return Err("chunk_size must be between 1 and 4194304".into());
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    runtime.block_on(async {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| e.to_string())?;
        // Avoid transfer compression layered on the already-gzipped file.
        let request = client
            .get(url)
            .header(reqwest::header::ACCEPT_ENCODING, "identity")
            .send();
        let Some(response) = waiting(request, adapter, controls).await else {
            return Ok(());
        };
        let mut response = response
            .map_err(|e| e.to_string())?
            .error_for_status()
            .map_err(|e| e.to_string())?;
        let mut prefix = Vec::new();
        while prefix.len() < 2 {
            let Some(chunk) = waiting(response.chunk(), adapter, controls).await else {
                return Ok(());
            };
            match chunk.map_err(|e| e.to_string())? {
                Some(bytes) => prefix.extend_from_slice(&bytes),
                None => break,
            }
        }
        let input = Input {
            adapter,
            controls,
            chunk_size,
        };
        let mut decoder = if prefix.starts_with(&[0x1f, 0x8b]) {
            Decoder::Gzip(MultiGzDecoder::new(input))
        } else {
            Decoder::Plain(input)
        };
        if let Err(e) = decoder.write(&prefix) {
            return if controls.is_stopped() {
                Ok(())
            } else {
                Err(e.to_string())
            };
        }
        loop {
            let Some(chunk) = waiting(response.chunk(), decoder.input().adapter, controls).await
            else {
                return Ok(());
            };
            let Some(bytes) = chunk.map_err(|e| e.to_string())? else {
                break;
            };
            if let Err(e) = decoder.write(&bytes) {
                return if controls.is_stopped() {
                    Ok(())
                } else {
                    Err(e.to_string())
                };
            }
        }
        let result = decoder.finish();
        // A seek/close can interrupt the decompressor while it emits its final
        // buffered output, just as it can interrupt an ordinary chunk write.
        if controls.is_stopped() {
            Ok(())
        } else {
            result
        }
    })
}
