pub mod frag_hdr;
pub mod packet_hdr;

// # Packet header
//
// ```text
// 0       2               6         (BYTE)
// +-------+---------------+
// | rwnd  |     nack      |
// +-------+---------------+
// ```
//
// # Fragment header
//
// ```text
// 0               4   5           8 (BYTE)
// +---------------+---+
// |      seq      |cmd|
// +---------------+---+
// |  len (Push)   |
// +---------------+---------------+
// |                               |
// |          DATA (Push)          |
// |                               |
// +-------------------------------+
// ```
//
// # Packet structure
//
// ```text
// [packet header]
// ([fragment header (Ack)])
// ([fragment header (Ack)])
// ([fragment header (Push)] [body])
// ([fragment header (Push)] [body])
// ```
//
// # Invariants
//
// - `len` (`Push`) should not be `0`
