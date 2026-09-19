# Deployment agent contract

Read [the deployment guide](README.md) and the repository [security
checklist](../docs/OPERATOR_SECURITY.md) before changing this directory.

- Keep master and validator in separate Compose files. The master service has
  profile `master`; the validator file contains no gateway or challenge service.
- All application images and the Python base image are `repository@sha256`.
  Never add `latest`, a mutable release tag or a placeholder that can boot.
- Master/validator containers run UID/GID 65532, read-only, with all capabilities
  dropped and `no-new-privileges`. They use a bounded `/tmp` tmpfs and a durable
  state volume.
- Credentials are read-only bind-mounted files. Do not add secret environment
  values, Docker socket mounts, host namespaces or privileged application roles.
- The validator mounts wallets and peer identity only. The master mounts gateway
  and challenge signing material only. Neither role receives VM-host provider
  keys.
- The Firecracker host is a systemd service on a KVM-capable machine. Its config
  requires explicit TLS, token, kernel/rootfs pins, resource ceilings and egress.
  Production has no fake-hypervisor or host-process fallback.
- Experiment limits never exceed 16 vCPU or 32 GiB RAM. An oversized request or
  host setting fails instead of being clamped.
- `scripts/check_deploy.py --check-examples` is the executable contract. Add a
  focused regression before changing a validated invariant.
- CI may render Compose and build containers. It must not deploy a host, contact
  OpenRouter, rent Lium, boot Firecracker or submit Bittensor weights.
- Do not commit materialized env files, state, wallets, certificates, keys,
  offers, rootfs images, kernels, packs or retained VM output.

When a setting changes, update its `.example`, `scripts/check_deploy.py`, tests
and `docs/reference/configuration.md` together.
