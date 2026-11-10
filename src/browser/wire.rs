use parity_scale_codec::{Decode, DecodeAll, Encode};
use wasm_bindgen::{JsCast, JsValue};

use crate::Error;

/// Version of the transport envelope, independent of operation names.
pub const VERSION: u32 = 2;

#[derive(Encode, Decode)]
/// One ordered request on a worker connection.
pub struct Request {
    /// Envelope version expected by the caller.
    pub version: u32,
    /// Correlation identifier, with zero reserved for startup.
    pub id: u64,
    /// Requested action.
    pub command: Command,
}

#[derive(Encode, Decode)]
/// Control messages and registered operation dispatch.
pub enum Command {
    /// Confirms initialization and protocol compatibility.
    #[codec(index = 0)]
    Hello,
    /// Executes one registered operation.
    #[codec(index = 1)]
    Call { name: String, input: Vec<u8> },
    /// Releases storage after earlier requests complete.
    #[codec(index = 2)]
    Close,
}

#[derive(Encode, Decode)]
/// Completion correlated to one request.
pub struct Reply {
    /// Envelope version used by the worker.
    pub version: u32,
    /// Identifier supplied by the caller.
    pub id: u64,
    /// Encoded operation output or an explicit failure.
    pub result: Result<Vec<u8>, Error>,
}

/// Encodes a transport envelope as a structured-clone-compatible byte array.
pub fn encode(value: &impl Encode) -> JsValue {
    js_sys::Uint8Array::from(value.encode().as_slice()).into()
}

/// Rejects malformed or extended transport envelopes.
pub fn decode<Value: Decode>(value: JsValue) -> Result<Value, Error> {
    if !value.is_instance_of::<js_sys::Uint8Array>() {
        return Err(Error::Protocol("expected a binary worker message".into()));
    }
    let bytes = js_sys::Uint8Array::new(&value).to_vec();
    Value::decode_all(&mut bytes.as_slice()).map_err(|error| Error::Protocol(error.to_string()))
}

/// Formats an exception received from the browser API.
pub fn js_error(value: JsValue) -> String {
    value.as_string().unwrap_or_else(|| format!("{value:?}"))
}
