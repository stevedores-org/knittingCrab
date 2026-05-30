# Kustomize manifests for knitting-crab-coordinator

K8s packaging for `crates/coordinator` per the org-wide kustomize convention.

## Structure

```
kustomize/
├── base/
│   ├── namespace.yaml        # knitting-crab-system
│   ├── deployment.yaml       # Coordinator deployment (hardened)
│   ├── service.yaml          # ClusterIP (ports 4000, 8080)
│   ├── configmap.yaml        # Default env vars
│   └── kustomization.yaml    # Base aggregation
└── overlays/
    ├── dev/                  # Single replica, debug logging
    └── prod/                 # 3 replicas, info logging
```

## Deployment

The coordinator is containerized via `dockworker.toml` and deployed via Flux.
Workers remain on bare-metal Apple Silicon and connect to the coordinator via its ClusterIP/DNS name.
