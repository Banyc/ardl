//! # Packet header
//!
//! ```text
//! 0       2               6         (BYTE)
//! +-------+---------------+
//! | rwnd  |     nack      |
//! +-------+---------------+
//! ```
//!
//! # Fragment
//!
//! ```text
//! 0               4   5           8 (BYTE)
//! +---------------+---+
//! |      seq      |cmd|
//! +---------------+---+
//! |  len (Push)   |
//! +---------------+---------------+
//! |                               |
//! |          Body (Push)          |
//! |                               |
//! +-------------------------------+
//! ```
//!
//! # Packet structure
//!
//! ```text
//! (Packet header)
//! (Fragment header of type Ack)*
//! (Fragment header of type Ack)*
//! ((Fragment header of type Push) (Body))*
//! ((Fragment header of type Push) (Body))*
//! ```
//!
//! # Invariants
//!
//! - `len` (`Push`) should not be `0`

pub mod frag;
pub mod packet;
pub mod packet_hdr;

#[derive(Debug)]
pub enum DecodingError {
    Decoding { field: &'static str },
}

#[derive(Debug)]
pub enum EncodingError {
    NotEnoughSpace,
}
