# Build a Proof guest rootfs

This procedure turns the `guest` target in `deploy/Dockerfile` into the exact
ext4 bytes that the Firecracker host pins. Run it on an isolated Linux builder
with Docker or Podman, Python 3.12, `fakeroot`, `e2fsprogs` and enough free space for the
selected image size. The builder needs root, or a rootless Podman user namespace
where its mapped user is root, so numeric ownership in the OCI export remains
correct.

## Build and measure

From the repository root:

```bash
sudo deploy/guest/bake-rootfs.sh --out-dir /var/tmp/cortex-guest
```

The script builds only the Python `guest` stage, captures its content-addressed
image ID, exports it without starting a container, rejects unsafe archive
members, and creates ext4 without mounting it. It prints four values:

```text
rootfs=/var/tmp/cortex-guest/sha256-<rootfs-digest>.ext4
digest=sha256:<rootfs-digest>
source_image=sha256:<oci-image-id>
source_date_epoch=946684800
```

`fakeroot` lets the exporter preserve every numeric OCI owner even inside a
restricted user namespace. The fixed source date, deterministic filesystem
UUID, normalized inode `ctime` and eager inode/journal initialization make
repeated builds from the same OCI image reproducible when the OCI engine and
e2fsprogs version are unchanged. The final SHA-256 remains the authority across
different builders.

To convert an already reviewed local image ID or registry digest without
rebuilding the Docker stage:

```bash
sudo deploy/guest/bake-rootfs.sh \
  --image sha256:<64-hex-image-id> \
  --out-dir /var/tmp/cortex-guest
```

The script accepts only a local `sha256:<hex>` ID or an exact
`repository@sha256:<hex>` reference. It refuses tags and existing files whose
bytes do not match their digest-based name.

## Install the artifact

Verify and install the output on the dedicated VM host:

```bash
sha256sum /var/tmp/cortex-guest/sha256-<rootfs-digest>.ext4
sudo install -o root -g root -m 0644 \
  /var/tmp/cortex-guest/sha256-<rootfs-digest>.ext4 \
  /var/lib/proof/images/sha256-<rootfs-digest>.ext4
sha256sum /var/lib/proof/images/sha256-<rootfs-digest>.ext4
```

Add the raw 64-hex digest and installed absolute path to `[images]` in the
private VM-host configuration. Set `PROOF_RLM_VM_IMAGE_DIGEST` on the master to
the same digest with the `sha256:` prefix. Record the separately measured guest
kernel digest in `[host].kernel_digest`; the rootfs builder does not build or
invent a kernel pin.

Before opening a topic, create and destroy a test VM through the authenticated
orchestrator API and run one dedicated experiment. A successful image build
does not prove that the host kernel, jailer, networking or teardown path works.
