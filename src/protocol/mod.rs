//! # Packet header
//!
//! ```text
//! 0       2               6         (BYTE)
//! +-------+---------------+
//! | rwnd  |     nack      |
//! +-------+---------------+
//! ```
//!
//! # Fragment header
//!
//! ```text
//! 0               4   5           8 (BYTE)
//! +---------------+---+
//! |      seq      |cmd|
//! +---------------+---+
//! |  len (Push)   |
//! +---------------+---------------+
//! |                               |
//! |          DATA (Push)          |
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

pub mod frag_hdr;
pub mod packet_hdr;
