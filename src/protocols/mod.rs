pub mod stream_frag;
pub mod stream_frag_hdr;
pub mod stream_packet_hdr;

// # Packet header
//
// ```text
// 0       2   3   4   5   6       8 (BYTE)
// +-------+---------------+
// |  wnd  |     nack      |
// +-------+---------------+
// ```
//
// # Fragment header
//
// ```text
// 0       2   3   4   5   6       8 (BYTE)
// +---------------+---+
// |     seq       |cmd|
// +---------------+---+
// |  len (Push)   |
// +---------------+---------------+
// |                               |
// |          DATA (Push)          |
// |                               |
// +-------------------------------+
// ```
