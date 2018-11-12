# The Simple UDP Transport Protocol

This document describes the Simple UDP Transport Protocol. It is very nice.

## Introduction

The SUTP defines a reliable transport protocol suitable for cases in which, for example, TCP would commonly be used. For quicker iteration on the protocol, however, SUTP is based on UDP datagrams instead of IP packets.

This RFC uses terminology as defined in [[RFC 2119]](https://tools.ietf.org/html/rfc2119).

## Data Layout

The data format is specified in https://laboratory.comsys.rwth-aachen.de/sutp/data-format.

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
