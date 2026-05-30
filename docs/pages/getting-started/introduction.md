---
title: "Introduction"
description: "What Edge Python is and where to go next."
---

# Introduction

Welcome to the Edge Python docs! 👋 Edge is a sandboxed subset of Python, compiled to a less than 200 KB WebAssembly binary and built in Rust to run on Cloudflare Workers and in the browser. Embed your full business logic, run LLMs client-side, build frontend apps and serverless workloads.

## Ecosystem

1. [Quickstart](/getting-started/quickstart): Run your first Edge Python program in under a minute.
2. [Syntax](/language/syntax): How to write a program?
3. [Reference](/reference/builtins): All the builtin methods.
4. [Implementation](/implementation/design): Compiler architecture, dispatch model, and runtime layout.

## Try it

### Browser:

Run live in your browser at [demo.edgepython.com](https://demo.edgepython.com/).

### Command Line Interface:

Or download it to your machine ([reference docs](/reference/builtins)):

```bash
curl -fsSL https://dylan-sutton-chavez.github.io/edge-python/install.sh | sh

edge -h # List all commands
```
