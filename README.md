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

The current high-level direction is:

```text
Trasolve
   |
   | HTTP / API
   v
troute
   |
   +-- Routing API
   +-- Route Solver
   +-- Graph / Optimization
```

This architecture is preliminary and may change as the project develops.

## Tech Stack

- Rust

Planned:

- HTTP API
- JSON-based requests and responses

## Development Roadmap

- [ ] Basic Rust project setup
- [ ] Point and route data models
- [ ] Distance matrix generation
- [ ] Simple greedy route solver
- [ ] 2-opt or similar local optimization
- [ ] External routing data integration
- [ ] Constraint-aware route optimization

## Getting Started

Local build and run instructions are not available yet. The repository does not currently contain a Rust package manifest or executable source code.

## Branch

The main development branch is `main`.

## Project Status

Early development. The project structure and core functionality have not been implemented yet.
