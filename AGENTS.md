# knittingCrab — Agent Capabilities

## SchedulerAgent

The **SchedulerAgent** (Coordinator) is the central orchestration service. It manages task dependencies, resource allocation, and remote worker assignment.

### Capabilities

1. **DAG-aware Task Scheduling**
   - Resolves complex task dependencies in a Directed Acyclic Graph.
   - Prevents cycles and ensures deterministic execution order.
   - Respects task priorities (Critical, High, Normal, Low).

2. **Resource Management**
   - Tracks CPU, RAM, and Metal (GPU) availability across local and remote nodes.
   - Allocates resources to tasks based on demand and availability.

3. **Remote Orchestration**
   - Manages remote sessions on `aivcs.local` via SSH and tmux.
   - Synchronizes code and state via `aivcs`.

## WorkerAgent

The **WorkerAgent** is the execution engine that runs tasks on specific hardware.

### Capabilities

1. **Local Process Execution**
   - Spawns and monitors local subprocesses.
   - Handles graceful termination and signal forwarding.

2. **Lease Management**
   - Acquires exclusive leases to prevent duplicate task execution.
   - Performs heartbeat renewals to maintain lease validity.

3. **Remote Task Bridge**
   - Executes tasks within isolated tmux sessions on remote hosts.
   - Polls for completion via sentinel files.

## A2A Protocol

| Direction | Event | Purpose |
|-----------|-------|---------|
| Inbound | `TASK_SUBMITTED` | Enqueue a new task into the DAG |
| Outbound | `TASK_COMPLETED` | Notify that a task and its dependents are finished |
| Peer | `LEASE_ACQUIRED` | Coordinate execution across multiple workers |
