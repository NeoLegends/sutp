# The Simple UDP Transport Protocol

This document describes the Simple UDP Transport Protocol. It is very nice.

----

## Introduction

The SUTP defines a reliable transport protocol suitable for cases in which, for example, TCP would commonly be used. For quicker iteration on the protocol, however, SUTP is based on UDP datagrams instead of IP packets.

This RFC uses terminology as defined in [[RFC 2119]](https://tools.ietf.org/html/rfc2119).

----

## Data Layout

SUTP divides the stream into segments, that each consist of a base header and chunks. A segment is a self-describing unit of data and control instructions within SUTP. Each segment is transferred within its own UDP packet. A chunk can contain user or protocol control data.

A segment MUST begin with the base header. It MUST contain at least one chunk. All numerical values MUST be represented in network order (i. e. big-endian).

### Example segment structure

```
|----------------|
|  Base Header   |
|----------------|
|    Chunk #1    |
|       |        |
|       |        |
|      ...       |
|----------------|
|      ...       |
|      ...       |
|      ...       |
|----------------|
|    Chunk #n    |
|       |        |
|       |        |
|      ...       |
|----------------|

|     32 bit     |
```

### Base Header Layout

`|<32 bit> sq no|<32 bit> window size|`

#### Field Explanation

1. Sequence number of the segment.
1. Size of the receiving window. The size includes the total bytes the receiver is willing to accept, including base header and chunks.

### Chunks

A chunk MUST begin with a chunk header. A chunk header consists of a 16 bit number that identifies the type of the chunk and a 16 bit number that specifies the length of the payload in bytes _excluding_ length of the chunk header and _excluding_ padding. Each chunk MUST be padded to a size multiple of 4 bytes by inserting 0-3 0-bytes at the end. If not specified, chunks MAY occur multiple times in a single segment.

Total layout: `|<16 bit> type of chunk|<16 bit> length of chunk payload|<length bytes> payload|<0-3 byte> padding|`

#### Chunk Support Requirements

An SUTP implementation MUST support the following chunks:

- `0x0`: [Payload Chunk](#chunk-payload)
- `0x1`: [SYN Chunk](#chunk-syn)
- `0x2`: [FIN Chunk](#chunk-fin)
- `0x3`: [ABRT Chunk](#chunk-abrt)
- `0x4`: [SACK Chunk](#chunk-sack)

It MAY support other chunk types. An implementation that does not support a chunk of a given type MUST ignore it.

#### Payload Chunk <a name="chunk-payload"></a>

Type: `0x0`

The chunk contains the raw payload data of the upper layer. The data MUST NOT be interpreted in any way.

##### Chunk Layout

`|<length bytes>data payload|`

#### SYN Chunk <a name="chunk-syn"></a>

Type: `0x1`

This chunk is present during connection set up. It always has a length of 0 and just its presence is enough.

There MUST be at most one chunk of this type in any segment.

#### FIN Chunk <a name="chunk-fin"></a>

Type: `0x2`

This chunk is present during connection tear down up. When it is present it means the sender of the chunk will close its sending channel, but is still ready to receive more data.

There MUST be at most one chunk of this type in any segment.

#### ABRT Chunk <a name="chunk-abrt"></a>

Type: `0x3`

This chunk is present when an error has occured. Sending it signals immediate connection shutdown of both sides. No further data may be sent.

Upon reception, the receiver MUST NOT in turn answer with a segment containing an ABRT chunk.

There MUST be at most one chunk of this type in any segment.

#### Selective Ack Chunk <a name="chunk-sack"></a>

Type: `0x4`

The chunk contains selective ack (SACK) data for reliability. All packets up to the first sequence number in the lists are cumulatively ACK'ed.

##### Chunk Layout

`|<16 bit> ack list len|<16 bit> nack list len|[<32 bit> ack seq no]|[<32 bit> nack seq no]|`

##### Field Explanation

1. Length (count) of ACK list
2. Length (count) of NAK list
3. List of ACK'd seq nos
4. List of NAK'd seq nos

#### Security Flag Chunk <a name="chunk-sec"></a>

Type: `0xfe`

This chunk defines whether the segment contains malicious data. There MUST be at most one security flag chunk in each segment.

##### Chunk Layout

`|<7 bit> zeroes|<1 bit> EVL flag|`

##### Field Explanation

1. 7 bit MUST be set to 0.
1. EVL bit according to [[RFC 3514]](https://tools.ietf.org/html/rfc3514).

----

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
