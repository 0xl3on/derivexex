# derivexex


## Motivation
Inspired by [this](https://www.paradigm.xyz/2024/05/reth-exex) great Paradigm article, I've decided to build this minimal Rollup Derivation Pipeline specifically for [Unichain](https://www.unichain.org/), even though it can be easily abstracted to be usable across other op-stack L2's. This is merely for fun and should not be used in prd!

## What is a Exex?

By own Paradigm's definition, that is repeated across their articles and docs, an Exex is basically a [Future](https://doc.rust-lang.org/std/future/trait.Future.html) that runs alongside Reth, where it's futures are polled.

## Sources

* https://github.com/paradigmxyz/reth-exex-examples
* https://github.com/paradigmxyz/reth/
* https://www.paradigm.xyz/2024/05/reth-exex
* https://docs.unichain.org/
* https://etherscan.io/address/0xFf00000000000000000000000000000000000130
* https://etherscan.io/address/0x2f60a5184c63ca94f82a27100643dbabe4f3f7fd