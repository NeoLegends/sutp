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

### List of chunks not being specifically acknowledged <a name="no-ACK-List"></a>

1. `SACK Chunk`


Reliability in SUTP is accomplished by using 'SACK Chunk' and timeouts.

At the beginning of a new connection a sending timeout, maximum waiting time and a receiving timeout are defined.  
The sending timeout determines how long a sending side waits for an acknowledegment, before sending the segment again.  
The maximum waiting time determines how long a sending side waits for an acknowledgement before the connection will be aborted.  It ultimately determines how often a certain segment can be sent again.  
The receiving timout determines how long a receiving side waits for an unsuccesfully sent segment before it negativelyy acknowledges it.  
The maximum waiting time SHOULD ne greater than the sending timeout and the sending timeout SHOULD be greater than the roundtriptime.

A SUTP instance can be both sending and receiving side at the same time.

### Receiving Side

A segment that was incorrectly (incorrect checksum) received is discarded without any further action.

The last segment's sequence number, to which all preceding segments were successfully received, is used for the cumulative acknowledgment.  If there have been subsequent segments successfully received, the the segments in between that were not succesfully received yet, are now handled as unsuccessfully sent.  A timer (receiving timeout) is set for every unsuccessfully sent segment.  If it has not been succesfully received on timeout, its sequence number is added to the NAK-List.  
After every received segment containing more than chunks of this [list](#no-ACK-List) and on every timeout for an unsuccefully sent segment a SACK-CHUNK built as described above, MUST be sent to the sending SUTP.  This way segments only containing chunks of this [list](#no-ACK-List) are not directly acknowledged (to avoid ACK loops), if received correctly. But they are eventually handled as unsuccessfully sent, if incorrectly (incorrect checksum) received.


### Sending Side

The sending side MUST have a segment ready to be sent again until it has been acknowledged or the connection is forcibly closed.

For a segment only containing chunks of this [list](#no-ACK-List) a timer (sending timer) is set, if no negative acknowledement has been received on timeout, the segment is indirectly acknowleded.  
For every other sent segment two timers (sending timeout and maximum waiting time) are set.  If the maximum waiting time timeout occurs before the segment has been acknowledged, the connection MUST be aborted.  
A segment must be sent again if no acknowledement has been received before the sending timeout occurs or when a negative acknowledgement was received.


Every segment is directly/indirectly negatively or positevely acknowledged in an appropriate time.  If negatively acknowledged or not acknowledged at all, the segment is sent again, therefore a reliable communication is guarenteed.


## Flow Control

SUTP is a flow-controlled protocol that tries to avoid overflowing the receiver with data.  This is accomplished by utilizing a sliding-window mechanism to make the sender adapt its send rate without introducing too much overhead.  The window size of one side is specified in a dedicated header field.

An instance of SUTP MUST have a receive buffer into which all incoming data is buffered until further processing of the upper layer has occured.  The size of the receive buffer is initially chosen during connection setup.  On a normal computer or smartphone, the authors recommend a size of at least 8MB.  To account for machine load, the size of the receive buffer MAY vary at runtime.

The receiving side MUST specify the amount of total bytes it is currently willing to accept without futher processing in the window size field in each segment.  The sending size SHOULD NOT at a time send more data than whatever is the last known value of the receiver's window size, without any of the data being ACKed or NAKed.

The receiver MUST always only consider the last known value of the window size field for its calculations.


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
- Current sequence number `sA` of type `integer`
- Sequence number of the other side `sB` of type `integer`
- Destination address `addr` of type `ip`
- Source and destination port numbers `srcPort` and `dstPort` of type `integer`

### Abort the session <a name="action-abort-session"></a>

Given:
- The current sequence number `sA`
- Receiving window `r` of type `integer`
- Destination address `addr` of type `ip`
- Destination port numbers`dstPort` of type `integer`

...aborting the session is done as follows:

1. Let `ch` be a list of chunks containing the ABRT chunk
1. [`Send a segment`](#action-send-segment) using chunk list `ch`, current sequence number `sA`, receiving window `r`, destination address `addr` and port `dstPort`
1. Close receiving and sending UDP sockets

### Get a new sequence number <a name="action-incr-sequence"></a>

Given:
- The current sequence number `sA`

...getting a new sequence number is done as follows:

1. Let `x` be an integer with value `(n + 1) % 2^32`
1. Set the current sequence number to `x`
1. Return `x`

### Send a segment <a name="action-send-segment"></a>

Given:
- The receiving window `r`
- The current sequence number `sA`
- A list of chunks to transfer `ch`
- Destination address `addr`
- Source port `srcPort` and destination port `dstPort`

...sending a segment containing the chunks is done as follows:

1. Let `x` be the result of [`Get a new sequence number`](#action-incr-sequence).
1. Let `buf` be the result of [`Serialize a segment`](#action-serialize-segment) using sequence number `x`, receiving window `r` and list of chunks `ch`.
1. [`Transfer a segment`](#action-transfer-segment) using binary buffer `buf`, destination address `addr` and destination port `dstPort`.

### Serialize a segment <a name="action-serialize-segment"></a>

Given:
- A sequence number `sA`
- The receiving window `r`
- A list of chunks to transfer `ch`

...serializing a segment is done as follows:

1. Write segment contents to an empty binary buffer `buf`
    1. Write base header to buffer using sequence number `sA` and receiving window `r`
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


## Session Lifecycle

This chapter describes how an SUTP session between two parties is initialized, held-up during data transfer phase and shutdown in the end.

### Session initiation

Let A be the initiator of the session and B the initiatee.

Given destination address `addrB` and port number `pB` of B, initiating a new SUTP connection between A and B is done as follows:

1. A chooses a random sequence number `sA`, a receiving window size `rA` and initializes variable `sB` (sequence number of B) to be of type `integer`
1. A opens a UDP listener on a random port number `pA`. If this fails A MUST stop initializing the connection and immediately report the error to the upper layer.
1. A compiles a new list of chunks `ch1` containing at least the `SYN Chunk` and optionally initialization chunks as required by protocol extensions
1. Set a timeout `tInit` covering the total connection initiation
    - If `tInit` elapses before the connection can be initiated, A MUST stop initializing the connection and immediately report the error to the upper layer
1. Let A [`Send a segment`](#action-send-segment) (in the following called `s1`) using chunk list `ch1`, `addrB`, `pA`, `pB`, `sA` and `rA`. This fragment is what is referred to by (SYN ->)
1. Set a timeout for B's answer `tB`
    - If B does not respond within the timeout, A MUST stop initializing the connection and immediately report the error to the upper layer
    - Otherwise continue normally
1. A parses the received data into an SUTP segment `s2`. This is what is referred to by (SYN <-)
1. Ensure `s2` is a valid SUTP segment and contains a `SYN Chunk` and a `SACK Chunk` `chSack`
    - If not, A MUST [`Abort the session`](#action-abort-session) and report the error to the upper layer
    - Otherwise continue normally
1. Ensure `chSack` ACKs `s1`
    - If it does not, go to step 5
    - Otherwise continue normally
1. Let `sB` be `s2`'s sequence number
1. If present, initialize any protocol extensions
1. The connection is now initialized.  Proceed with `Data Transfer`-phase.

### Data Transfer

Let A and B be the both SUTP session endpoints.

If A wishes to send data `d` to B it must:

1. Check what B's last `window size` is
    - If the window size is larger than the payload A wishes to send, A may proceed
    - Otherwise A must wait until B has informed A about free window space (usually by responding with ACK / NAK for some packets)
1. Potentially apply compression to `d` and compile payload chunk containing `d`
1. Subtract `d`s size from the local copy of B's window size
1. Set a timer `tAbort` guarding against complete protocol or network failure.  The authors recommend a value of at least 30 seconds for a regular wired internet connection, a larger value for wireless or mobile networks.
    - If `tAbort` elapses, A MUST [`Abort the session`](#action-abort-session) and report the error to the upper layer
1. A [`Sends a segment`](#action-send-segment) to B containing the compiled payload chunk
1. Set a timer `tTimeout`.  The authors recommend a value of 1 second for all types of connections.
    - If `tTimeout` elapses, continue with step 4.
1. Wait for B's answer.
    - If it contains a SACK-Chunk, see if it ACKs or NAKs the previously sent segment.
    - If not, proceed with step 7.
1. If B's answer ACKs the previously sent segment, cancel `tTimeout` and `tAbort` and report success to the upper layer.
1. If B's answer NAKs the previously sent segment, check whether the window size still allows sending the segment.
    - If it does, continue with step 4.
    - If it does not, wait until B reports free window space, then continue with step 4.

When A receives a segment `s` containing payload data from B it must:

1. Check whether A's current remaining window size allows processing `s` and whether the CRC-32 sum matches
    - If it does not, [`Send a segment`](#action-send-segment) containing a NAK to B and return.
    - If it does, continue normally
1. Tag `s` with ACK
1. Subtract the size of `s` from the current window size
1. Check if the window size has become close to 0 (by some implementation-specific margin)
    - If it does, immediately [`Send a segment`](#action-send-segment) ACKing `s` and potentially other successfully received segments and report the current window size.  Return
    - If it does not, continue normally
1. Apply a debounce mechanism for all events leading up to here.  The authors recommend debouncing to 200ms
1. [`Send a segment`](#action-send-segment) containing a SACK chunk for all outstanding segments to be ACKed and NAKed.

### Connection Shutdown

1. -> FIN Sending channel closed
1. <- FIN + SACK Receiving channel closed

### Connection Abort

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
