#!/usr/bin/env python3
"""Smoke-test an AIHelper release ZIP using only its extracted contents."""

import argparse
import json
import os
from pathlib import Path, PurePosixPath
import queue
import signal
import stat
import subprocess
import sys
import tempfile
import threading
import time
import zipfile


PLUGIN_DOMAINS = ("github", "gitlab", "ollama", "postgres")
SENTINEL_TOOLS = {
    "github": "ah.github.repo",
    "gitlab": "ah.gitlab.project",
    "ollama": "ah.ollama.ask",
    "postgres": "ah.postgres.tool.status",
}
COMMAND_TIMEOUT_SECONDS = 15
MCP_RESPONSE_TIMEOUT_SECONDS = 10
MCP_STOP_TIMEOUT_SECONDS = 2


class SmokeError(Exception):
    """Expected release smoke failure."""


def platform_layout():
    if sys.platform == "win32":
        return "ah.exe", ".dll", "ah-update-helper.exe"
    if sys.platform == "darwin":
        return "ah", ".dylib", None
    if sys.platform.startswith("linux"):
        return "ah", ".so", None
    raise SmokeError("unsupported smoke-test platform")


def normalized_member_name(name):
    path = PurePosixPath(name.replace("\\", "/"))
    if (
        not name
        or path.is_absolute()
        or any(part in ("", ".", "..") for part in path.parts)
        or (path.parts and ":" in path.parts[0])
    ):
        raise SmokeError("archive contains an unsafe member path")
    return path.as_posix()


def extract_archive(archive, destination):
    executable_name, library_suffix, helper_name = platform_layout()
    expected = {executable_name}
    if helper_name is not None:
        expected.add(helper_name)
    expected.update(
        "plugins/ah-plugin-{}{}".format(domain, library_suffix)
        for domain in PLUGIN_DOMAINS
    )

    try:
        with zipfile.ZipFile(str(archive)) as bundle:
            members = {}
            equivalent_members = set()
            for member in bundle.infolist():
                normalized = normalized_member_name(member.filename)
                equivalent = normalized.casefold()
                if equivalent in equivalent_members:
                    raise SmokeError("archive contains duplicate member paths")
                equivalent_members.add(equivalent)
                if stat.S_ISLNK(member.external_attr >> 16):
                    raise SmokeError("archive contains a symbolic link")
                members[normalized] = member

            missing = sorted(expected.difference(members))
            if missing:
                raise SmokeError("archive is missing: {}".format(", ".join(missing)))
            non_files = sorted(name for name in expected if members[name].is_dir())
            if non_files:
                raise SmokeError(
                    "expected archive files are directories: {}".format(
                        ", ".join(non_files)
                    )
                )
            bundle.extractall(str(destination))
    except zipfile.BadZipFile as error:
        raise SmokeError("archive is not a valid ZIP file") from error

    executable = destination / executable_name
    if os.name != "nt":
        executable.chmod(executable.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
    helper = None if helper_name is None else destination / helper_name
    return executable, helper


def smoke_version(executable, environment, cwd):
    try:
        completed = subprocess.run(
            [str(executable), "--version"],
            cwd=str(cwd),
            env=environment,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            timeout=COMMAND_TIMEOUT_SECONDS,
            check=False,
        )
    except subprocess.TimeoutExpired as error:
        raise SmokeError("version check timed out") from error
    except OSError as error:
        raise SmokeError("could not start the extracted executable") from error
    if completed.returncode != 0 or completed.stderr:
        raise SmokeError("version check failed")
    output = completed.stdout.rstrip("\r\n")
    if "\r" in output or "\n" in output or not output.startswith("ah "):
        raise SmokeError("version check returned an invalid identity")
    version = output[3:]
    if not version:
        raise SmokeError("version check returned an empty version")
    return version


def smoke_helper(helper, expected_version, environment, cwd):
    try:
        completed = subprocess.run(
            [str(helper), "--self-check"],
            cwd=str(cwd),
            env=environment,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            timeout=COMMAND_TIMEOUT_SECONDS,
            check=False,
        )
    except subprocess.TimeoutExpired as error:
        raise SmokeError("update helper self-check timed out") from error
    except OSError as error:
        raise SmokeError("could not start the update helper") from error
    if completed.returncode != 0 or completed.stderr:
        raise SmokeError("update helper self-check failed")
    try:
        response = json.loads(completed.stdout)
    except (TypeError, ValueError) as error:
        raise SmokeError("update helper self-check returned invalid JSON") from error
    expected = {
        "schema_version": 1,
        "protocol_version": 1,
        "helper_version": expected_version,
        "target": "x86_64-pc-windows-msvc",
        "architecture": "x86_64",
    }
    if type(response) is not dict or response != expected:
        raise SmokeError("update helper self-check returned an incompatible identity")


def smoke_plugins(executable, environment, cwd):
    try:
        completed = subprocess.run(
            [str(executable), "plugins", "list", "--json"],
            cwd=str(cwd),
            env=environment,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            timeout=COMMAND_TIMEOUT_SECONDS,
            check=False,
        )
    except subprocess.TimeoutExpired as error:
        raise SmokeError("plugins list timed out") from error
    except OSError as error:
        raise SmokeError("could not start the extracted executable") from error

    if completed.returncode != 0:
        raise SmokeError("plugins list exited with code {}".format(completed.returncode))
    try:
        plugins = json.loads(completed.stdout)
    except (TypeError, ValueError) as error:
        raise SmokeError("plugins list did not return valid JSON") from error
    if not isinstance(plugins, list):
        raise SmokeError("plugins list JSON must be an array")
    if any(not isinstance(plugin, dict) for plugin in plugins):
        raise SmokeError("plugins list JSON contains a non-object entry")

    for domain in PLUGIN_DOMAINS:
        matches = [plugin for plugin in plugins if plugin.get("domain") == domain]
        if len(matches) != 1:
            raise SmokeError(
                "expected one '{}' plugin, found {}".format(domain, len(matches))
            )
        plugin = matches[0]
        expected_fields = {
            "source": "dynamic",
            "state": "enabled",
            "mcp_exposed": True,
        }
        for field, expected_value in expected_fields.items():
            if plugin.get(field) != expected_value:
                raise SmokeError(
                    "plugin '{}' has unexpected '{}' value".format(domain, field)
                )


def read_lines(stream, output):
    try:
        for line in iter(stream.readline, ""):
            output.put(line)
    finally:
        output.put(None)


def drain_lines(stream):
    for _line in iter(stream.readline, ""):
        pass


def send_message(process, message):
    try:
        process.stdin.write(json.dumps(message, separators=(",", ":")) + "\n")
        process.stdin.flush()
    except (BrokenPipeError, OSError) as error:
        raise SmokeError("MCP server closed stdin unexpectedly") from error


def receive_response(process, responses, expected_id):
    deadline = time.monotonic() + MCP_RESPONSE_TIMEOUT_SECONDS
    while True:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise SmokeError("MCP response {} timed out".format(expected_id))
        try:
            line = responses.get(timeout=remaining)
        except queue.Empty as error:
            raise SmokeError("MCP response {} timed out".format(expected_id)) from error
        if line is None:
            code = process.poll()
            detail = "" if code is None else " with code {}".format(code)
            raise SmokeError("MCP server closed stdout{}".format(detail))
        try:
            message = json.loads(line)
        except (TypeError, ValueError) as error:
            raise SmokeError("MCP stdout contained invalid JSON") from error
        if not isinstance(message, dict):
            raise SmokeError("MCP response must be a JSON object")
        if message.get("jsonrpc") != "2.0":
            raise SmokeError("MCP response has an invalid JSON-RPC version")
        if "id" not in message:
            continue
        response_id = message.get("id")
        if type(response_id) is not type(expected_id) or response_id != expected_id:
            raise SmokeError("MCP returned an unexpected response id")
        if "error" in message:
            raise SmokeError("MCP response {} returned an error".format(expected_id))
        if not isinstance(message.get("result"), dict):
            raise SmokeError("MCP response {} has no result object".format(expected_id))
        return message["result"]


def stop_process(process):
    if process.stdin is not None:
        try:
            process.stdin.close()
        except OSError:
            pass
    try:
        process.wait(timeout=MCP_STOP_TIMEOUT_SECONDS)
        return
    except subprocess.TimeoutExpired:
        pass

    try:
        if os.name == "nt":
            process.terminate()
        else:
            os.killpg(process.pid, signal.SIGTERM)
    except (OSError, ProcessLookupError):
        pass
    try:
        process.wait(timeout=MCP_STOP_TIMEOUT_SECONDS)
        return
    except subprocess.TimeoutExpired:
        pass

    try:
        if os.name == "nt":
            process.kill()
        else:
            os.killpg(process.pid, signal.SIGKILL)
    except (OSError, ProcessLookupError):
        pass
    try:
        process.wait(timeout=MCP_STOP_TIMEOUT_SECONDS)
    except subprocess.TimeoutExpired as error:
        raise SmokeError("MCP server could not be stopped") from error


def smoke_mcp(executable, environment, cwd):
    popen_options = {
        "cwd": str(cwd),
        "env": environment,
        "stdin": subprocess.PIPE,
        "stdout": subprocess.PIPE,
        "stderr": subprocess.PIPE,
        "text": True,
        "encoding": "utf-8",
        "bufsize": 1,
    }
    if os.name == "nt":
        popen_options["creationflags"] = subprocess.CREATE_NEW_PROCESS_GROUP
    else:
        popen_options["start_new_session"] = True

    try:
        process = subprocess.Popen(
            [str(executable), "mcp", "serve", "--default-timeout-ms", "5000"],
            **popen_options
        )
    except OSError as error:
        raise SmokeError("could not start the extracted MCP server") from error

    responses = queue.Queue()
    stdout_thread = threading.Thread(
        target=read_lines, args=(process.stdout, responses), daemon=True
    )
    stderr_thread = threading.Thread(
        target=drain_lines, args=(process.stderr,), daemon=True
    )
    stdout_thread.start()
    stderr_thread.start()
    failure = None
    try:
        send_message(
            process,
            {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-11-25",
                    "capabilities": {},
                    "clientInfo": {"name": "release-smoke", "version": "1.0.0"},
                },
            },
        )
        initialized = receive_response(process, responses, 1)
        if initialized.get("protocolVersion") != "2025-11-25":
            raise SmokeError("MCP negotiated an unexpected protocol version")
        send_message(
            process,
            {"jsonrpc": "2.0", "method": "notifications/initialized"},
        )
        send_message(
            process,
            {"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}},
        )
        result = receive_response(process, responses, 2)
        tools = result.get("tools")
        if not isinstance(tools, list):
            raise SmokeError("tools/list did not return a tools array")
        names = {
            tool.get("name")
            for tool in tools
            if isinstance(tool, dict) and isinstance(tool.get("name"), str)
        }
        missing = [name for name in SENTINEL_TOOLS.values() if name not in names]
        if missing:
            raise SmokeError("tools/list is missing: {}".format(", ".join(missing)))
    except BaseException as error:
        failure = error
    finally:
        try:
            stop_process(process)
        except SmokeError as error:
            if failure is None:
                failure = error
        for stream in (process.stdout, process.stderr):
            try:
                stream.close()
            except OSError:
                pass
        stdout_thread.join(timeout=MCP_STOP_TIMEOUT_SECONDS)
        stderr_thread.join(timeout=MCP_STOP_TIMEOUT_SECONDS)
    if failure is not None:
        raise failure


def parse_args():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", type=Path, help="completed release ZIP path")
    return parser.parse_args()


def main():
    args = parse_args()
    archive = args.archive.resolve()
    if not archive.is_file():
        raise SmokeError("release archive does not exist")

    with tempfile.TemporaryDirectory(prefix="aihelper-release-smoke-") as temp:
        root = Path(temp)
        extracted = root / "archive"
        extracted.mkdir()
        config_dir = root / "config"
        config_dir.mkdir()
        executable, helper = extract_archive(archive, extracted)
        environment = os.environ.copy()
        environment["AH_CONFIG_DIR"] = str(config_dir)
        version = smoke_version(executable, environment, extracted)
        if helper is not None:
            smoke_helper(helper, version, environment, extracted)
        smoke_plugins(executable, environment, extracted)
        smoke_mcp(executable, environment, extracted)
    print("release smoke passed: {}".format(archive.name))


if __name__ == "__main__":
    try:
        main()
    except SmokeError as error:
        print("release smoke failed: {}".format(error), file=sys.stderr)
        sys.exit(1)
    except Exception as error:
        print(
            "release smoke failed: unexpected {}".format(type(error).__name__),
            file=sys.stderr,
        )
        sys.exit(1)
