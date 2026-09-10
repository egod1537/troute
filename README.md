# troute

Routing and route optimization engine for Trasolve.

## Overview

troute is an independent server intended to handle routing and itinerary optimization computations for Trasolve. It separates computation-heavy, algorithm-focused work from the Trasolve frontend and general service logic.

The project is expected to address problems such as:

- Computing routes between multiple places
- Optimizing visit order
- Evaluating routes by distance and travel time
- Optimizing itineraries with time constraints

These capabilities are planned and are not yet implemented.

## Goals

- Generate distance matrices
- Optimize routes and visit order
- Explore TSP- and VRP-family problems
- Support time-window constraints
- Account for opening hours and stay duration
- Support user-defined constraints
- Use travel-time data from real-world mapping services

The long-term goal is not only to provide shortest-path calculations, but to develop an optimization engine that can account for the constraints involved in real travel itineraries.

## Architecture

The current high-level structure is:

```text
Trasolve
   |
   | HTTP / API
   v
troute
   |
   +-- API types
   +-- Routing Provider
   +-- Travel Time Matrix
   +-- Route Solver
   +-- Schedule Calculation
```

The component interfaces and v0 data flow are defined, but concrete HTTP,
Google Maps, caching, and solver implementations have not been selected.

## Tech Stack

- Rust
- Serde

Planned:

- HTTP API

## Development Roadmap

- [x] Basic Rust project setup
- [x] v0 request, response, location, and route data models
- [x] Travel time matrix model
- [x] Routing provider and solver interfaces
- [x] Schedule calculation for a supplied visit order
- [ ] Distance matrix generation
- [ ] Simple greedy route solver
- [ ] 2-opt or similar local optimization
- [ ] External routing data integration
- [ ] Constraint-aware route optimization

## Getting Started

Build the library and run its tests with:

```sh
cargo build
cargo test
```

There is no runnable HTTP server yet.

## Branch

The main development branch is `main`.

## Project Status

Early development. The v0 types and component boundaries are in place; routing
provider, optimization algorithm, and HTTP server implementations are pending.
