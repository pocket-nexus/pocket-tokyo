//! Ranges of a file: over HTTP in a browser tab, from disk elsewhere.
//!
//! A pack is one file a runtime reads parts of: its tables at the start, then
//! a record when the eye comes near it. In a tab each read is a request with a
//! `Range` header, so the server must answer ranges (status 206).

use std::cell::Cell;
use std::rc::Rc;

/// What has been read so far.
#[derive(Clone, Copy, Debug, Default)]
pub struct Read {
    pub requests: u32,
    pub bytes: u64,
}

#[derive(Clone)]
pub struct Source {
    /// A URL in a tab, a path elsewhere.
    place: Rc<str>,
    read: Rc<Cell<Read>>,
}

impl Source {
    pub fn new(place: &str) -> Source {
        Source { place: place.into(), read: Rc::new(Cell::new(Read::default())) }
    }

    pub fn place(&self) -> &str {
        &self.place
    }

    /// Requests made and bytes received since the start.
    pub fn read_so_far(&self) -> Read {
        self.read.get()
    }

    fn count(&self, bytes: usize) {
        let was = self.read.get();
        self.read.set(Read { requests: was.requests + 1, bytes: was.bytes + bytes as u64 });
    }

    /// `size` bytes from `offset`.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn range(&self, offset: u64, size: u64) -> Result<Vec<u8>, String> {
        use std::io::{Read as _, Seek, SeekFrom};
        let mut file = std::fs::File::open(&*self.place).map_err(|e| format!("{}: {e}", self.place))?;
        file.seek(SeekFrom::Start(offset)).map_err(|e| format!("{}: {e}", self.place))?;
        let mut bytes = vec![0u8; size as usize];
        file.read_exact(&mut bytes).map_err(|e| format!("{}: {e}", self.place))?;
        self.count(bytes.len());
        Ok(bytes)
    }

    /// `size` bytes from `offset`.
    #[cfg(target_arch = "wasm32")]
    pub async fn range(&self, offset: u64, size: u64) -> Result<Vec<u8>, String> {
        use wasm_bindgen::JsCast;
        use wasm_bindgen_futures::JsFuture;
        let said = |what: &str, e: wasm_bindgen::JsValue| format!("{}: {what}: {}", self.place, e.as_string().or_else(|| js_sys::Error::from(e).message().as_string()).unwrap_or_default());
        let init = web_sys::RequestInit::new();
        let headers = web_sys::Headers::new().map_err(|e| said("headers", e))?;
        headers.set("Range", &format!("bytes={}-{}", offset, offset + size - 1)).map_err(|e| said("headers", e))?;
        init.set_headers(&headers);
        let request = web_sys::Request::new_with_str_and_init(&self.place, &init).map_err(|e| said("request", e))?;
        // (a tab's window, or a worker's scope)
        let global = js_sys::global();
        let pending = match global.dyn_ref::<web_sys::Window>() {
            Some(window) => window.fetch_with_request(&request),
            None => global.unchecked_ref::<web_sys::WorkerGlobalScope>().fetch_with_request(&request),
        };
        let response: web_sys::Response = JsFuture::from(pending).await.map_err(|e| said("no answer", e))?.unchecked_into();
        // A server that sends the whole file for a range would have a tab download all of a pack for each read.
        if response.status() != 206 {
            return Err(format!("{}: status {} for a range (the server must answer byte ranges with 206)", self.place, response.status()));
        }
        let buffer = JsFuture::from(response.array_buffer().map_err(|e| said("body", e))?).await.map_err(|e| said("body", e))?;
        let bytes = js_sys::Uint8Array::new(&buffer).to_vec();
        if bytes.len() as u64 != size {
            return Err(format!("{}: {} bytes for a range of {size}", self.place, bytes.len()));
        }
        self.count(bytes.len());
        Ok(bytes)
    }
}
