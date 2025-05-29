# rust-proxy

A lightweight TCP proxy written in Rust, designed for simplicity and performance.
Features

    TCP Proxying: Forwards TCP connections from a local port to a specified remote address.

    Asynchronous I/O: Utilizes Rust's asynchronous capabilities for efficient handling of multiple connections.

    Minimal Dependencies: Focuses on core functionality with minimal external dependencies.

Getting Started
Prerequisites

    Rust (latest stable version recommended)

Building the Project

Clone the repository:

git clone https://github.com/shricubed/rust-proxy.git
cd rust-proxy

Build the project:

cargo build --release

Running the Proxy

After building, you can run the proxy with the following command:

cargo run --release -- <LOCAL_PORT> <REMOTE_HOST> <REMOTE_PORT>

Replace <LOCAL_PORT> with the port you want the proxy to listen on, <REMOTE_HOST> with the destination host, and <REMOTE_PORT> with the destination port.

For example, to forward local port 8080 to example.com on port 80:

cargo run --release -- 8080 example.com 80

