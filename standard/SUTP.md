# The Simple UDP Transport Protocol

This document describes the Simple UDP Transport Protocol. It is very nice.

----

## Introduction

The SUTP defines a reliable transport protocol suitable for cases in which, for example, TCP would commonly be used. For quicker iteration on the protocol, however, SUTP is based on UDP datagrams instead of IP packets.

This RFC uses terminology as defined in [[RFC 2119]](https://tools.ietf.org/html/rfc2119).

----

## Data Layout

SUTP divides the stream into segments, that each consist of a base header and chunks. A segment is a self-describing unit of data and control instructions within SUTP. Each segment is transferred within its own UDP packet. A chunk can contain user or protocol control data.

All segments MUST consist of at least one chunk. All numerical values MUST be represented in network order (i. e. big-endian).

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

`|<8 bit> flags|<24 bit> sq no|<32 bit> window size|`

#### Field Explanation

1. Control flags (from highest to lowest bit).
    1. SYN - interpret according to the [[three-way-handshake]](#handshake).
    1. FIN - interpret according to the [[three-way-handshake]](#handshake).
    1. Unused - MUST be 0.
    1. Unused - MUST be 0.
    1. Unused - MUST be 0.
    1. Unused - MUST be 0.
    1. Unused - MUST be 0.
    1. Unused - MUST be 0.
1. Sequence number of the segment.
1. Size of the receiving window.

### Chunks

A chunk MUST begin with a chunk header. A chunk header consists of a 16 bit number that identifies the type of the chunk and a 16 bit number that specifies the length of the payload in bytes _excluding_ length of the chunk header and _excluding_ padding. Each chunk MUST be padded to a size multiple of 4 bytes by inserting 0-3 0-bytes at the end.

`|<16 bit> type of chunk|<16 bit> length of chunk payload|<length bytes> payload|<0-24 bit> padding|`

#### Payload Chunk <a name="chunk-payload"></a>

Type: `0x0`

The chunk contains the raw payload data of the upper layer. The data MUST NOT be interpreted in any way.

##### Chunk Layout

`|<length bytes> data payload|`

#### Selective Ack Chunk <a name="chunk-sack"></a>

Type: `0x1`

The chunk contains selective ack / nack data for reliability. There MUST be at most one selective ack chunk in each segment.

##### Chunk Layout

`|[<24 bit> seq no|<7 bit> zeroes|<1 bit> ACK / NAK]|`

##### Field Explanation

1. List of triples of:
    1. 24 bit sequence number.
    1. 7 bit MUST be set to zeroes.
    1. 1 bit ACK (= 1) or NAK (= 0) sequence number status.

#### Security Flag Chunk <a name="chunk-sec"></a>

Type: `0xfe`

This chunk defines whether the segment contains malicious data. There MUST be at most one security flag chunk in each segment.

##### Chunk Layout

`|<7 bit> zeroes|<1 bit> EVL flag|`

##### Field Explanation

1. 7 bit MUST be set to 0.
1. EVL bit according to [[RFC 3514]](https://tools.ietf.org/html/rfc3514).

----
