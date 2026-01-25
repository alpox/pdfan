# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

pdfan is a Rust-based PDF generation service that uses Chrome WebDriver (via fantoccini) to render web pages and convert them to PDF. The project is in early development and uses Rust 2024 edition.

## Build Commands

```bash
cargo build          # Build the project
cargo run            # Run the application (requires chromedriver installed)
cargo test           # Run tests
cargo check          # Type-check without building
```

## Dependencies

The project requires `chromedriver` to be installed and available in PATH. The application spawns chromedriver on port 4444.

## Architecture

### Core Components

- **driver.rs**: Process supervision system with traits for managing external processes
  - `Driver` trait: Defines how to start a process (e.g., `ChromeDriver`)
  - `Process` trait: Defines stop/wait operations on running processes
  - `Supervisor`: Manages process lifecycle with automatic restart on failure, uses broadcast channels for shutdown signaling

- **worker.rs**: Work queue system (incomplete)
  - `Producer<T>`: Manages a bounded async channel for tasks
  - `Worker<T>`: Consumes tasks from the queue
  - `Task` trait: Defines processable work items
  - `Stamped` trait: Provides identity for tasks

- **main.rs**: Entry point with payload types for PDF generation
  - `ChromeDriverPdfPayload`: HTML/URL to PDF conversion options
  - `TypstDriverPdfPayload`: Typst content rendering (planned)
  - Currently demonstrates WebDriver usage with Wikipedia navigation

### Concurrency Model

Uses tokio for async runtime with:
- `broadcast` channels for supervisor signaling
- `async-channel` for work queues
- `tokio::select!` for handling multiple async events
