"""Export the vendored Python lock without resolving or upgrading dependencies."""
import sys
import tomllib
from pathlib import Path

lock = tomllib.loads(Path(sys.argv[1]).read_text())
for package in lock["package"]:
    if "registry" not in package["source"]:
        continue
    wheels = package.get("wheels", [])
    if not wheels:
        raise ValueError(f"No locked wheels for {package['name']}")
    marker = "; sys_platform == 'win32'" if package["name"] == "pywin32" else ""
    hashes = " ".join(f"--hash={wheel['hash']}" for wheel in wheels)
    print(f"{package['name']}=={package['version']}{marker} {hashes}")
