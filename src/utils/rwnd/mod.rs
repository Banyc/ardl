mod recv_buf;
mod rwnd;
pub use recv_buf::*;

pub enum SeqLocationToRwnd {
    InRecvWindow,
    AtRecvWindowStart,
    TooLate,
    TooEarly,
}
