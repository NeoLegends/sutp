# The Simple UDP Transport Protocol

## Abstract

This document proposes the Simple UDP Transport Protocol (SUTP).  SUTP is a reliable transport protocol tunneled over UDP that provides the following:

- Acknowledged, error-free, non-duplicated and in-order transfer of payload data
- Data fragmentation to conform to path MTU limits
- Future-proof wire format


## Introduction

The SUTP defines a reliable transport protocol suitable for cases in which, for example, TCP would commonly be used.  For quicker iteration on the protocol, however, SUTP is based on UDP datagrams instead of IP packets.


## Requirements Language

This RFC uses terminology as defined in [RFC 2119](https://tools.ietf.org/html/rfc2119).


## In-Order Arrival

Every SUTP segment has a sequence number as described in [Data Layout](#data-layout).  A sequence number is unique in a given connection and time context.
This means that, on each SUTP side, a sequence number MUST be assigned to at most one unacknowledged segment at a time.  Sequence numbers can be reused, but only if every previous segment containing such a sequence number has been acknowledged by the receiver.

After the sequence number for the first segment was chosen, the sequence numbers of subsequent segments increase monotonically by `1` (see Data Layout).

Payload data MUST be transferred in order. This requires that payload data MUST be written to the payload chunks in the order it was received from the upper layer.

A SUTP instance MUST always pass data of `payload chunk`s to the upper layer in the order (by sequence number) of the segments the `payload chunk`s were received in.  It MUST NOT pass data of the same `payload chunk` more than once.  If a segment contains more than one `payload chunk`, the SUTP instance MUST pass the data of all `payload chunk`s to the upper layer in the order they were written in the segment.

So all in all, In-Order Arrival in SUTP is realised by sequence numbers and a strict order of `payload chunk`s in single segment.


## Reliability

Reliability in SUTP is accomplished by using 'SACK Chunk' and timeouts.  At the begining of a new connection a sending timeout and maximum waiting time are defined.  The sending timeout determines how long a sending side waits for an ACK for a segment after it was sent, before sending it again.  The maximum waiting time determines how long a sending side waits for an ACK for a segment after it was sent before the connection will be aborted.  It ultimately determines how often a certain segment can be sent again.  A SUTP instance is both sending and receiving side at the same time.

### Receiving Side

Sequence numbers of segments that contain only the 'SACK Chunk' are always tagged with an ACK but they are not be acknowledged by the receiving side sending an additional ACK (to avoid ACK loops).

For other kind of segments the receiving side MUST act as follows:  When a receiving side receives a segment with a correct checksum, the sequence number is tagged with an ACK.  When it receives one with an incorrect checksum the sequence number is tagged with a NAK.  Then the receiving side checks what the last ACKed sequence number is, to which all preceding sequence numbers are tagged with an ACK as well.  This last sequence number is the first one to be written in the 'SACK Chunk'-ACK list (cumulative ACK) the rest is added to the 'SACK Chunk' NAK list according to their tag.  After that the chunk may be added to a segment, if it is going to be sent immediately, or otherwise to a new segment and is sent to the sending side.

This procedure of acknowledging on the receiving side guarentees that every received segment, that contains more than just the SUTP header and the 'SACK Chunk' will be directly acknowledged or negatively acknowledged depending on it's checksum.

### Sending Side

The sending side must have a segment ready to be sent again until it received an ACK for it's sequence number (meanwhile more segments can be sent) or the connection is forcibly closed.

When the sending side sends a segment, a timer for this specific segment is set.  If the sending side receives an ACK for the segment (may be covered by the cumulative ACK) before timout the timer will be ignored.  The same applies to the case of receiving a NAK but in that case the sending side must send the segment with the specific sequence number again and reset the timer.  If a timeout occurs the sending side must send the segment with the specific sequence number again.  The procedure of sending a segment again, should only be repeated while all repitions together do not take longer than the maximum waiting time.  If that is the case the connection must be aborted.

All in all every segment containing more than the 'SACK Chunk' is immediately (negativeley) acknowledged.  If negatively acknowledged or not acknowledged at all, the sender sends the segment again, therefore a reliable communication is guarenteed.


## Flow Control




## Interfaces

The following description of user commands to the SUTP are the minimum requirements to support interprocess communication.

### Connect

Format: CONNECT (foreign address, foreign port, options)
_returns_: connection name

This call causes the SUTP to establish a connection to the specified connection partner using the given internet address and port number, via a 3-way Handshake.  This call is of active nature as the calling process will be the connection initiator.  For passive listening see [LISTEN](#interface-listen).  `options` MAY be omitted.

Possible uses are:
- Specifying a maximum waiting time.  After not receiving any package from the connection partner for this amount of time, the connection will be forcefully closed, for security reasons.
- Specifying which extensions should be attempted to be used while establishing the connection as the receiving end may not support the desired extensions.

### Listen <a name="interface-listen"></a>

Format: LISTEN (pending list length)

This call wil cause the SUTP to listen for any incoming connection requests up to a maximum amount of pending connection requests depicted by _pending list length_.  If a maximum is present and reached, any further incoming connection request should not be responded to.

### Accept

Format: ACCEPT ()
_returns_: connection name

This command causes pending connection requests that have been received via LISTEN, to be dequeued and turned into full connections for interprocess communications.  It therefore establishes a new reliable connection to the requesting host and returns the new connection name to possibly be used in sending and receiving data.

### Send

Format: SEND (connection name, buffer address, data length)

This call causes the data contained inside the given buffer to be send via the given connection up to _data length_.  If the connection doesn't exist, the SEND call will be considered an error.

### Receive

Format: RECEIVE (connection name, buffer address, buffer length)
_returns_: received data length

This call will fill the specified buffer with received data that came from the given connection up to a maximum of _buffer length_.  The caller will be informed about the amount of data received, which may be less than the size of the provided buffer.  To prevent deadlocks, implementations should avoid blocking the caller if no data has been received.

### Close

Format: CLOSE (connection name)

This command causes the specified connection to be closed.  Pending data should still get send to its destination to ensure reliability and data should still get received until the other side closes the connection as well.  Thus closing a connection should be understood as a one sided process.  For immediate abort of a connection see [ABORT](#interface-abort).

### Abort <a name="interface-abort"></a>

Format: ABORT (connection name)

This command causes all pending SENDs and RECEIVEs to be aborted and the specified connection to be closed forcefully.  A special ABORT-chunk is to be sent to inform the other side.


## Data Layout <a name ="data-layout"></a>

The data format is specified in https://laboratory.comsys.rwth-aachen.de/sutp/data-format.


## Basic Procedures

This chapter defines basic protocol procedures that will be composed to the full protocol automaton later.

Each instance of the protocol has the following properties:

- Receiving window `r` of type `integer`
- Current sequence number `n` of type `integer`
- Sequence number of the oder side `sB` of type `integer`
- Destination address `addr` of type `ip`
- Source and destination port numbers `srcPort` and `dstPort` of type `integer`

### Abort the session <a name="action-abort-session"></a>

Given:
- The current sequence number `n`
- Receiving window `r` of type `integer`
- Destination address `addr` of type `ip`
- Destination port numbers`dstPort` of type `integer`

...aborting the session is done as follows:

1. Let `ch` be a list of chunks containing the ABRT chunk
1. [`Send a segment`](#action-send-segment) using chunk list `ch`, current sequence number `n`, receiving window `r`, destination address `addr` and port `dstPort`
1. Close receiving and sending UDP sockets

### Get a new sequence number <a name="action-incr-sequence"></a>

Given:
- The current sequence number `n`

...getting a new sequence number is done as follows:

1. Let `x` be an integer with value `(n + 1) % 2^32`
1. Set the current sequence number to `x`
1. Return `x`

### Send a segment <a name="action-send-segment"></a>

Given:
- The receiving window `r`
- The current sequence number `n`
- A list of chunks to transfer `ch`
- Destination address `addr`
- Source port `srcPort` and destination port `dstPort`

...sending a segment containing the chunks is done as follows:

1. Let `x` be the result of [`Get a new sequence number`](#action-incr-sequence).
1. Let `buf` be the result of [`Serialize a segment`](#action-serialize-segment) using sequence number `x`, receiving window `r` and list of chunks `ch`.
1. [`Transfer a segment`](#action-transfer-segment) using binary buffer `buf`, destination address `addr` and destination port `dstPort`.

### Serialize a segment <a name="action-serialize-segment"></a>

Given:
- A sequence number `n`
- The receiving window `r`
- A list of chunks to transfer `ch`

...serializing a segment is done as follows:

1. Write segment contents to an empty binary buffer `buf`
    1. Write base header to buffer using sequence number `n` and receiving window `r`
    1. Write chunks to buffer
1. Compute CRC-32 of the current contents of `buf`
1. Write CRC-32 sum to buffer
1. Return `buf`

### Transfer a segment <a name="action-transfer-segment"></a>

Given:
- A binary buffer `buf`
- The destination ip address `addr`
- The destination port number `dstPort`

...transferring a segment is done as follows:

1. Send a UDP datagram to address `addr` and port `dstPort` containing `buf`


## Session initiation

Let A be the initiator of the session and B the initiatee.

Given destination address `addrB` and port number `pB` of B, initiating a new SUTP connection between A and B is done as follows:

1. A chooses a random sequence number `sA`, a receiving window size `rA` and initializes variable `sB` (sequence number of B) to be of type `integer`
1. A opens a UDP listener on a random port number `pA`. If this fails A MUST stop initializing the connection and immediately report the error to the upper layer.
1. A compiles a new list of chunks `ch1` containing at least the `SYN Chunk` and optionally initialization chunks as required by protocol extensions
1. Set a timeout `tInit` covering the total connection initiation
    - If `tInit` elapses before the connection can be initiated, A MUST stop initializing the connection and immediately report the error to the upper layer
1. Let A [`Send a segment`](#action-send-segment) (in the following called `s1`) using chunk list `ch1`, `addrB`, `pA`, `pB`, `sA` and `rA`
1. Set a timeout for B's answer `tB`
    - If B does not respond within the timeout, A MUST stop initializing the connection and immediately report the error to the upper layer
    - Otherwise continue normally
1. A parses the received data into an SUTP segment `s2`
1. Ensure `s2` contains a `SYN Chunk` and a `SACK Chunk` `chSack`
    - If not, A MUST [`Abort the session`](#action-abort-session) and report the error to the upper layer
    - Otherwise continue normally
1. Ensure `chSack` ACKs `s1`
    - If it does not, go to step 5
    - Otherwise continue normally
1. Let `sB` be `s2`'s sequence number
1. If present, initialize any protocol extensions
1. The connection is now initialized


## Connection Shutdown

1. -> FIN Sending channel closed
1. <- FIN + SACK Receiving channel closed


## Connection Abort

1. -> ABRT

Both channels closed


## Extensions

Extensions are functionality that is optional to the base functionality of SUTP.  Implementations MAY choose to support any number of extensions.  Extension support is purely optional and implementations with support for certain extensions MUST be backwards-compatible to implementations without extension support.

### Compression

An implementation of SUTP MAY support payload compression.

An implementation that does support compression MUST support at least two algorithms listed in the data format specification of the `SUTP Compression Negotiation Chunk`.

#### Initiating a compressed session

Let A be the initiator of the session and B the initiatee.

1. A compiles an `SUTP Compression Negotiation Chunk` with all compression algorithms A supports (in order of preference).
1. A sends this chunk within the first (SYN ->) segment to B.
1. Upon reception, B, if it supports compression, checks whether it supports any of the algorithms listed.
    1. If it does support at least one algorithm, B picks _one_ algorithm from the list and also compiles an `SUTP Compression Negotiation Chunk` containing just the ID of that algorithm.  For optimal results, B SHOULD try to obey the order of preference A has specified.  Then B sends this chunk to the sender within the second (SYN <-) segment.
    1. If B does not support any of the algorithms listed within the initial `SUTP Compression Negotiation Chunk` it MUST NOT send an `SUTP Compression Negotiation Chunk` back.
1. Upon receiving the second segment, A checks whether the segment contains an `SUTP Compression Negotiation Chunk`.
    1. If it does, A checks whether the chunk is valid.  A chunk is valid, if: a) the chunk contains at most one ID of a compression algorithm and b) A supports this algorithm.  If the chunk is invalid, a MUST [`Abort the session`](#action-abort-session).
    1. If it does not, or if that chunk is empty (i. e. contains no compression algorithms), A skips the next step and proceeds normally without applying compression.
1. A MUST now compress all _payload_ data using the algorithm B has picked before compiling the `Payload Chunk`.
1. If a compression algorithm has been negotiated, B MUST decompress the payload data before handing it to the upper layer using the algorithm it has chosen during negotiation.


## Authors' Addresses

- Constantin Buschhaus <constantin.buschhaus@rwth-aachen.de>
- Marvin Hohn <marvin.hohn@rwth-aachen.de>
- Moritz Gunz <moritz.gunz@rwth-aachen.de>
