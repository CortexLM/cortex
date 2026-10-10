"""Mock executable only: records pinned-kind argv, stops at first control-plane exec."""

import json
import os
import pathlib
import sys

args = sys.argv[1:]
trace = pathlib.Path(os.environ["KIND_MOCK_TRACE"])
state = pathlib.Path(os.environ["KIND_MOCK_STATE"])
with trace.open("a") as stream:
    stream.write(json.dumps(args) + "\n")
node = json.loads(state.read_text()) if state.exists() else None
op = args[0]
if op in {"-v", "--version", "version"}:
    print("Docker version 29.1.3, build mocked")
elif op == "info":
    print(json.dumps({"CgroupVersion": "2", "CgroupDriver": "systemd", "MemoryLimit": True}))
elif op == "ps":
    if node:
        print("hypertrain-v2-contract-control-plane")
elif op == "pull":
    print("PINNED_PULL_MOCKED")
elif op == "network":
    if args[1] == "ls":
        print("c" * 64)
    elif args[1] == "inspect":
        if "-f" in args or "--format" in args:
            print("172.18.0.0/16 " if "Subnet" in " ".join(args) else "1500")
        else:
            print(
                json.dumps(
                    [
                        {
                            "Id": "c" * 64,
                            "Name": "kind",
                            "Created": "2026-10-09T00:00:00Z",
                            "Labels": {},
                            "Containers": {},
                            "IPAM": {"Config": [{"Subnet": "172.18.0.0/16"}]},
                        }
                    ]
                )
            )
elif op == "inspect":
    if "--type=image" in args:
        sys.exit(1)
    if "--format" in args or "-f" in args:
        fmt = args[args.index("--format") + 1] if "--format" in args else args[args.index("-f") + 1]
        if "Role" in fmt or "role" in fmt:
            print("control-plane")
        elif "IPAddress" in fmt:
            print("172.18.0.2,")
        else:
            print("1")
    else:
        assert node is not None
        print(json.dumps([node]))
elif op == "create":
    assert "--cpus=3.5" in args and "--memory=4g" in args and "--memory-swap=4g" in args
    node = {
        "Id": "a" * 64,
        "State": {"Running": False, "Pid": 0},
        "HostConfig": {
            "NanoCpus": 3500000000,
            "Memory": 4 << 30,
            "MemorySwap": 4 << 30,
            "CpusetCpus": "0-3",
        },
        "Config": {
            "Labels": {
                "io.hypertrain.proof": "hypertrain-v2-contract",
                "io.x-k8s.kind.cluster": "hypertrain-v2-contract",
            }
        },
    }
    state.write_text(json.dumps(node))
    print(node["Id"])
elif op == "start":
    journal = json.loads(pathlib.Path(os.environ["KIND_MOCK_JOURNAL"]).read_text())
    assert (
        journal["node_capped_before_start"] and journal["create_host_config"] == node["HostConfig"]
    )
    node["State"]["Running"] = True
    node["State"]["Pid"] = 12345
    state.write_text(json.dumps(node))
    print(node["Id"])
elif op == "logs":
    receipt = json.loads(pathlib.Path(os.environ["KIND_MOCK_JOURNAL"]).read_text())
    assert receipt["node_kernel_receipt"]["verified"]
    print("Reached target Multi-User System.")
    print("Reached target multi-user.target")
elif op == "exec":
    receipt = json.loads(pathlib.Path(os.environ["KIND_MOCK_JOURNAL"]).read_text())
    assert receipt["node_kernel_receipt"]["verified"]
    sys.stderr.write("MOCK_CONTROLPLANE_STOP_AFTER_CAPPED_START")
    sys.exit(73)
elif op == "rm":
    state.unlink(missing_ok=True)
elif op == "stop":
    assert args[-1] == node["Id"] and args[1] == "--time=0"
    node["State"] = {"Running": False, "Pid": 0}
    state.write_text(json.dumps(node))
else:
    sys.stderr.write("UNIMPLEMENTED_MOCK_ARGV:" + json.dumps(args))
    sys.exit(74)
