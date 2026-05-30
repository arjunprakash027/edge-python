---
title: "Introduction"
description: "What Edge Python is and where to go next."
---

# Introduction

Edge Python is a sandboxed Python subset compiled to a less 200 KB WebAssembly module release, built in Rust to run on Cloudflare Workers, and the browser.

## Explore

1. [Quickstart](/getting-started/quickstart): Run your first Edge Python program in under a minute.
2. [The language](/language/syntax): How to write a program?
3. [Reference](/reference/builtins): All the builtin methods.
4. [Implementation](/implementation/design): Compiler architecture, dispatch model, and runtime layout.

## Try it

### Browser:

Run live in your browser at [demo.edgepython.com](https://demo.edgepython.com/).

### Command Line Interface:

Or download it to your machine ([reference docs](https://github.com/dylan-sutton-chavez/edge-python/tree/main/cli)):

```bash
curl -fsSL https://dylan-sutton-chavez.github.io/edge-python/install.sh | sh

edge -h # List all commands
```
