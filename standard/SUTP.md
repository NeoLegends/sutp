# The Simple UDP Transport Protocol

This document describes the Simple UDP Transport Protocol. It is very nice.

## Introduction

The SUTP defines a reliable transport protocol suitable for cases in which, for example, TCP would commonly be used. For quicker iteration on the protocol, however, SUTP is based on UDP datagrams instead of IP packets.

This RFC uses terminology as defined in [[RFC 2119]](https://tools.ietf.org/html/rfc2119).

## Data Layout

The data format is specified in https://laboratory.comsys.rwth-aachen.de/sutp/data-format.

## Basic Procedures

Each instance of the protocol has the following properties:

- Receiving window `r` of type `number`
- Current sequence number `n` of type `number`
- Destination address `addr` of type `ip`
- Source and destination port numbers `srcPort` and `dstPort` of type `number`

### Get a new sequence number <a name="action-incr-sequence"></a>

Given:
- The current sequence number `n`

...getting a new sequence number is done as follows:

1. Initialize variable `x` of type `number`
1. Let `x` be `n + 1`
1. Set the current sequence number to `x`
1. Return `x`

### Send a segment <a name="action-send-segment"></a>

Given:
- The receiving window `r`
- The current sequence number `n`
- A list of chunks to transfer `ch`
- Destination address `addr`
- Destination port `dstPort`

...sending a segment containing the chunks is done as follows:

1. Initialize variable `x` of type `number`
1. Initialize variable `buf` of type `binary buffer`
1. Let `x` be the result of [`Get a new sequence number`](#action-incr-sequence).
1. Let `buf` be the result of [`Serialize a segment`](#action-serialize-segment) using sequence number `x`, receiving window `r` and list of chunks `ch`.
1. [`Transfer a segment`](#action-transfer-segment) using binary buffer `buf`, destination address `addr` and destination port `dstPort`.

### Serialize a segment <a name="action-serialize-segment"></a>

Given:
- A sequence number `n`
- The receiving window `r`
- A list of chunks to transfer `ch`

...serializing a segment is done as follows:

1. Let `buf` be a variable of type `binary buffer`
1. Write segment contents to binary buffer
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

## Handshake

Three way handshake with the following chunks:

1: -> SYN Chunk + Multiple unspecified init chunks + Random Seq Nos
2: <- SYN Chunk + SACK Chunk ACKing 1 + Multiple unspecified init chunks
3: -> SACK Chunk ACKing 2 + Payload

## Shutdown

1: -> FIN Sending channel closed
1: <- FIN + SACK Receiving channel closed

## ABRT

1: -> ABRT

Both channels closed

## Interfaces

The following description of user commands to the SUTP are the minimum requirements to support interprocess communication.

### Connect

Format: CONNECT (foreign address, foreign port, options)   
_returns_: connection name

This call causes the SUTP to establish a connection to the specified connection partner using the given internet address and port number, via a 3-way Handshake. This call is of active nature as the calling process will be the connection initiator. For passive listening see [LISTEN](#interface-listen).
**Options** MAY be omitted. Possible uses are:  
Specifying a maximum waiting time. After not receiving any package from the connection partner for this amount of time, the connection will be forcefully closed, for security reasons.  
Specifying which extensions should be attempted to be used while establishing the connection as the receiving end may not support the desired extensions.

### Listen <a name="interface-listen"></a>

Format: LISTEN (pending list length)

This call wil cause the SUTP to listen for any incoming connection requests up to a maximum amount of pending connection requests depicted by _pending list length_. If a maximum is present and reached, any further incoming connection request should not be responded to.  

### Accept

Format: ACCEPT ()  
_returns_: connection name

This command causes pending connection requests that have been received via LISTEN, to be dequeued and turned into full connections for interprocess communications. It therefore establishes a new reliable connection to the requesting host and returns the new connection name to possibly be used in sending and receiving data.

### Send

Format: SEND (connection name, buffer address, data length)

This call causes the data contained inside the given buffer to be send via the given connection up to _data length_.  If the connection doesn't exist, the SEND call will be considered an error.

### Receive

Format: RECEIVE (connection name, buffer address, buffer length)  
_returns_: received data length

This call will fill the specified buffer with received data that came from the given connection up to a maximum of _buffer length_. The caller will be informed about the amount of data received, which may be less than the size of the provided buffer. To prevent deadlocks, implementations should avoid blocking the caller if no data has been received.

### Close

Format: CLOSE (connection name)

This command causes the specified connection to be closed. Pending data should still get send to its destination to ensure reliability and data should still get received until the other side closes the connection as well. Thus closing a connection should be understood as a one sided process. For immediate abort of a connection see [ABORT](#interface-abort).

### Abort <a name="interface-abort"></a>

Format: ABORT (connection name)

This command causes all pending SENDs and RECEIVEs to be aborted and the specified connection to be closed forcefully. A special ABORT-chunk is to be sent to inform the other side.