# Arc Gateway Kubernetes Deployment

This directory provides a baseline Kubernetes deployment for Arc Gateway.
It includes namespace setup, configuration, secrets, workload resources, and autoscaling.

## Files

- `namespace.yaml` creates the `arc-gateway` namespace.
- `configmap.yaml` provides the `arc.toml` template.
- `secret.yaml` provides token and TLS placeholders.
- `deployment.yaml` defines the main workload with rolling updates and graceful shutdown.
- `service.yaml` defines both internal and node level services.
- `hpa.yaml` configures horizontal autoscaling by CPU utilization.

## Apply order

```bash
kubectl apply -f deploy/kubernetes/namespace.yaml
kubectl apply -f deploy/kubernetes/secret.yaml
kubectl apply -f deploy/kubernetes/configmap.yaml
kubectl apply -f deploy/kubernetes/deployment.yaml
kubectl apply -f deploy/kubernetes/service.yaml
kubectl apply -f deploy/kubernetes/hpa.yaml
```

## Configuration notes

- Update `configmap.yaml` with your runtime values.
- Replace placeholders in `secret.yaml` before deployment.
- Keep `control_plane.auth_token` consistent with probe headers.
- `deployment.yaml` uses `Authorization: Bearer $(ARC_TOKEN)` for health probes.

## XDP note

XDP is disabled by default in this manifest set.
Optional XDP related security settings are included as comments in `deployment.yaml`.
Enable them only when your cluster nodes and operating model support eBPF and privileged networking features.
