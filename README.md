# derivexex
A minimal Rollup Derivation Pipeline built using Reth's ExEx.

## Table of Contents

- [Motivation](#motivation)
- [What is an ExEx?](#what-is-an-exex)
- [What are Ethereum Blobs?](#what-are-ethereum-blobs)
- [KZG Commitments and Versioned Hashes](#kzg-commitments-and-versioned-hashes)
- [The L1 and L2 Relationship](#the-l1-and-l2-relationship)
- [What is a Sequencing Epoch?](#what-is-a-sequencing-epoch)
- [What is a Batch?](#what-is-a-batch)
- [What are Optimism Channels?](#what-are-optimism-channels)
- [What are Frames?](#what-are-frames)
- [Sources](#sources)

## Motivation

Inspired by [this](https://www.paradigm.xyz/2024/05/reth-exex) great Paradigm article, I've decided to build this minimal Rollup Derivation Pipeline specifically for [Unichain](https://www.unichain.org/), even though it can be easily abstracted to be usable across other op-stack L2's. This is merely for fun and should not be used in prd!

## What is an ExEx?

An ExEx is basically a [Future](https://doc.rust-lang.org/std/future/trait.Future.html) that runs alongside Reth, where it's futures are polled.

## What are Ethereum Blobs?

Blobs (Binary Large Object) were introduced on Ethereum in Dencun fork (2024). They are a temporary (they are pruned from consensus after ~18 days, more below), cheaper way for Layer 2's to post data to the L1 and have a standard size of 128kb. Blobs contents are called `frames` (it's definition is just below).

## KZG Commitments and Versioned Hashes

Each blob (128kb) has a KZG commitment, a 48byte proof of its contents. The L1 execution layer doesn't store full blobs, only their versioned hashes. A versioned hash is derived from the KZG commitment and is what gets stored in EIP-4844 transactions. To fetch a blob from the beacon node (sidecar), you use the versioned hash as a lookup key.

## The L1 and L2 Relationship

Each L2 block is tied to an L1 block called its "L1 origin". `A L1 block can be the origin for other multiple L2 blocks`, it's also called Sequencing Epoch on Optimism spec.

## What is a Batch?

A batch is the data needed to build one L2 block. It contains an `epoch number, an L2 timestamp, and a list of transactions`. Batches are compressed together into [channels](#what-are-frames) for compression efficiency.

## What are Optimism Channels?

A channel is a sequence of batches compressed together. Compressing multiple batches as a group yields better compression ratios than compressing each individually. A channel is identified by a unique 16-byte ID and info about a certain L2 block `can be span across more than one L1 block`.

## What are Frames?

A frame is a chunk of a channel that fits into a blob. `Since blobs are limited to 128KB and channels can be larger, channels are split into ordered frames`. Each frame contains a channel ID, a frame number, payload data, and a flag indicating if it's the last frame. Once all frames arrive, the channel is reassembled and decompressed.

## Sources

* [reth-exex-examples (Github)](https://github.com/paradigmxyz/reth-exex-examples)
* [reth (Github)](https://github.com/paradigmxyz/reth/)
* [kona (Github)](https://github.com/op-rs/kona/)
* [reth-exex (Paradigm article)](https://www.paradigm.xyz/2024/05/reth-exex)
* [Unichain (Docs)](https://docs.unichain.org/)
* [BatchInbox (Etherscan)](https://etherscan.io/address/0xFf00000000000000000000000000000000000130)
* [Batcher (Etherscan)](https://etherscan.io/address/0x2f60a5184c63ca94f82a27100643dbabe4f3f7fd)
* [Optimism (Docs)](https://specs.optimism.io/)
