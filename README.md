# Aggressively Reliable Delivery Layer

![arch](img/arch.drawio.png)

## How to use

- Echo server - `src/bin/echo.rs`
- Interactive client - `src/bin/telnet_client.rs`

## Jargons

- wtr: writer
- rdr: reader
- recv: receive
- swnd: send window
- rwnd: receive window
- tx: transmit
- rx: receive
- frag: fragment
- hdr: header
- que: queue
- dup: duplicate
- buf: buffer
- seq: sequence
- nack: not acknowledged
- cmd: command
- len: length
- utils: utilities
- rto: retransmission timeout
- rtt: round trip time
- addr: address
