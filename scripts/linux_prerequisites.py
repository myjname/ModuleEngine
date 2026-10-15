#!/usr/bin/env python3
"""Read-only Linux prerequisites probe; never installs or changes settings."""
import argparse
import ctypes as c
import ctypes.util
import json
import os
from pathlib import Path
import platform
import shutil
import socket
import subprocess


def tool(name):
    path = shutil.which(name)
    child_environment = None
    if name == "dxc" and platform.system() == "Linux":
        bundled = Path(__file__).resolve().parent.parent / "tools/dxc/bin/dxc"
        path = os.environ.get("MODULE_ENGINE_DXC") or (str(bundled) if bundled.is_file() else path)
        if path:
            resolved = shutil.which(path) if not os.path.dirname(path) else path
            if not resolved:
                return {"path": path, "error": "Native DXC executable was not found on PATH"}
            try:
                executable = Path(resolved).resolve(strict=True)
                if not executable.is_file() or not executable.stat().st_mode & 0o111:
                    return {"path": path, "error": "Native DXC must be a regular executable file"}
                with executable.open("rb") as stream:
                    if stream.read(4) != b"\x7fELF":
                        return {"path": path, "error": "Native Linux DXC must be ELF; Windows PE/interop is unsupported"}
                path = str(executable)
            except (OSError, RuntimeError) as error:
                return {"path": path, "error": f"Native DXC validation: {error}"}
            parent = executable.parent
            libraries = (parent.parent if parent.name == "bin" else parent) / "lib"
            if libraries.is_dir():
                child_environment = os.environ.copy()
                child_environment["LD_LIBRARY_PATH"] = str(libraries) + (
                    ":" + child_environment["LD_LIBRARY_PATH"]
                    if child_environment.get("LD_LIBRARY_PATH") else "")
    result = {"path": path}
    if path:
        try:
            proc = subprocess.run([path, "--version"], capture_output=True, text=True,
                                  timeout=10, env=child_environment)
            result.update(exit_code=proc.returncode, version=(proc.stdout or proc.stderr).splitlines()[:2])
        except (OSError, subprocess.TimeoutExpired) as error:
            result["error"] = str(error)
    return result


def local_socket(path):
    result = {"path": str(path), "exists": path.exists(), "connectable": False}
    if result["exists"]:
        try:
            with socket.socket(socket.AF_UNIX) as connection:
                connection.settimeout(2)
                connection.connect(str(path))
            result["connectable"] = True
        except OSError as error:
            result["error"] = str(error)
    return result


def vulkan():
    result = {"loader": ctypes.util.find_library("vulkan"), "devices": []}
    if not result["loader"]:
        return result
    instance = c.c_void_p()
    lib = None
    try:
        lib = c.CDLL(result["loader"])

        class Application(c.Structure):
            _fields_ = [("sType", c.c_uint32), ("pNext", c.c_void_p),
                        ("application", c.c_char_p), ("applicationVersion", c.c_uint32),
                        ("engine", c.c_char_p), ("engineVersion", c.c_uint32),
                        ("apiVersion", c.c_uint32)]

        class Create(c.Structure):
            _fields_ = [("sType", c.c_uint32), ("pNext", c.c_void_p), ("flags", c.c_uint32),
                        ("application", c.POINTER(Application)), ("layerCount", c.c_uint32),
                        ("layers", c.c_void_p), ("extensionCount", c.c_uint32),
                        ("extensions", c.c_void_p)]

        application = Application(0, None, b"ModuleEngine prerequisite probe", 0, None, 0, 1 << 22)
        create = Create(1, None, 0, c.pointer(application), 0, None, 0, None)
        lib.vkCreateInstance.argtypes = [c.POINTER(Create), c.c_void_p, c.POINTER(c.c_void_p)]
        lib.vkCreateInstance.restype = c.c_int32
        result["create_result"] = lib.vkCreateInstance(c.byref(create), None, c.byref(instance))
        if result["create_result"] != 0:
            return result
        lib.vkEnumeratePhysicalDevices.argtypes = [c.c_void_p, c.POINTER(c.c_uint32), c.c_void_p]
        lib.vkEnumeratePhysicalDevices.restype = c.c_int32
        count = c.c_uint32()
        status = lib.vkEnumeratePhysicalDevices(instance, c.byref(count), None)
        result["enumerate_result"] = status
        if status != 0 or not count.value:
            return result
        devices = (c.c_void_p * count.value)()
        result["enumerate_result"] = lib.vkEnumeratePhysicalDevices(instance, c.byref(count), devices)
        lib.vkGetPhysicalDeviceProperties.argtypes = [c.c_void_p, c.c_void_p]
        lib.vkGetPhysicalDeviceProperties.restype = None
        for device in devices[:count.value]:
            # VkPhysicalDeviceProperties begins with five uint32 values,
            # deviceName[256], UUID[16]; reserve aligned space for remaining limits.
            storage = (c.c_uint64 * 512)()
            lib.vkGetPhysicalDeviceProperties(device, c.byref(storage))
            raw = bytes(storage)
            words = (c.c_uint32 * 5).from_buffer_copy(raw[:20])
            result["devices"].append({"name": raw[20:276].split(b"\0", 1)[0].decode(errors="replace"),
                                      "type": words[4], "api_version": words[0],
                                      "software": words[4] == 4})
    except (OSError, AttributeError) as error:
        result["error"] = str(error)
    finally:
        if instance.value and lib is not None:
            lib.vkDestroyInstance.argtypes = [c.c_void_p, c.c_void_p]
            lib.vkDestroyInstance.restype = None
            lib.vkDestroyInstance(instance, None)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--require", choices=["build", "gui", "hlsl"])
    args = parser.parse_args()
    result = {"platform": platform.platform(), "machine": platform.machine(),
              "linux": platform.system() == "Linux", "wsl": "microsoft" in platform.release().lower(),
              "environment": {key: os.environ.get(key) for key in
                              ["DISPLAY", "WAYLAND_DISPLAY", "WAYLAND_SOCKET", "XDG_RUNTIME_DIR"]},
              "tools": {name: tool(name) for name in ["rustc", "cargo", "cc", "pkg-config", "dxc"]}}
    if result["linux"]:
        result["libraries"] = {name: ctypes.util.find_library(name) for name in
                               ["vulkan", "wayland-client", "xkbcommon", "X11"]}
        wayland = os.environ.get("WAYLAND_DISPLAY")
        if wayland:
            path = Path(wayland)
            if not path.is_absolute():
                path = Path(os.environ.get("XDG_RUNTIME_DIR", "/nonexistent")) / path
            result["wayland"] = local_socket(path)
        display = os.environ.get("DISPLAY", "")
        if display.startswith(":"):
            number = display[1:].split(".", 1)[0]
            result["x11"] = local_socket(Path("/tmp/.X11-unix") / ("X" + number))
        result["vulkan"] = vulkan()
    missing = []
    if args.require:
        if not result["linux"]:
            missing.append("native Linux environment")
        if args.require in ["build", "gui"]:
            missing.extend(name for name in ["rustc", "cargo", "cc", "pkg-config"]
                           if result["tools"][name].get("exit_code") != 0)
        if args.require == "gui":
            if not result.get("vulkan", {}).get("devices"):
                missing.append("usable Vulkan device")
            if not any(result.get(name, {}).get("connectable") for name in ["wayland", "x11"]):
                missing.append("connectable Wayland/X11 socket")
        if args.require == "hlsl":
            if result["tools"]["dxc"].get("exit_code") != 0:
                missing.append("usable native DXC and its libraries (MODULE_ENGINE_DXC, tools/dxc/bin/dxc, or PATH)")
    result["missing_required"] = missing
    result["scope"] = "Prerequisites only; does not build or execute ModuleEngine or authenticate a display."
    print(json.dumps(result, ensure_ascii=False, indent=2))
    return 2 if missing else 0


if __name__ == "__main__":
    raise SystemExit(main())
