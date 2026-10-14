"""Verify an installed signed release update and rollback on a fresh Windows CI VM."""
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import urllib.request

from release_smoke import extract_archive, smoke_helper

PREVIOUS = "2.0.1"
CURRENT = "2.0.2"
REPOSITORY = "Bobsans/AIHelper"


def run(executable, arguments, environment):
    result = subprocess.run(
        [str(executable), *arguments],
        env=environment,
        stdin=subprocess.DEVNULL,
        capture_output=True,
        text=True,
        timeout=180,
    )
    if result.returncode != 0:
        raise RuntimeError(f"{arguments}: {result.stdout}\n{result.stderr}")
    return result.stdout.strip()


def helper_running():
    result = subprocess.run(
        ["powershell", "-NoProfile", "-NonInteractive", "-Command",
         "(Get-Process -Name ah-update-helper -ErrorAction SilentlyContinue | Measure-Object).Count"],
        capture_output=True, text=True, check=True, timeout=15,
    )
    return int(result.stdout.strip()) != 0


def wait_installed(executable, state, expected, environment):
    deadline = time.monotonic() + 180
    while time.monotonic() < deadline:
        manifests = list((state / "installations").glob("*/installed.manifest.json"))
        if len(manifests) == 1 and not helper_running():
            path = manifests[0]
            manifest = json.loads(path.read_text(encoding="utf-8"))
            if manifest["release"]["version"] == expected:
                assert path.with_name("installed.manifest.sig").is_file()
                version = run(executable, ["--version"], environment)
                if version == f"ah {expected}":
                    return path.parent.name
        time.sleep(1)
    raise RuntimeError(f"Installed version did not reach {expected} before the deadline")


def main():
    assert sys.platform == "win32" and os.environ.get("GITHUB_ACTIONS") == "true"
    state = Path(os.environ["APPDATA"]) / "AIHelper" / "updater"
    assert not state.exists(), "Acceptance requires a fresh runner profile"
    root = Path(os.environ["RUNNER_TEMP"]) / "aihelper-upgrade-v202"
    root.mkdir()
    request = urllib.request.Request(
        f"https://api.github.com/repos/{REPOSITORY}/releases/tags/v{PREVIOUS}",
        headers={"Accept": "application/vnd.github+json", "User-Agent": "AIHelper-release-acceptance"},
    )
    with urllib.request.urlopen(request, timeout=60) as response:
        release = json.load(response)
    assert release["tag_name"] == f"v{PREVIOUS}" and not release["draft"] and not release["prerelease"]
    asset = next(item for item in release["assets"] if item["name"] == "ah-windows-x64.zip")
    url = asset["browser_download_url"]
    assert url == f"https://github.com/{REPOSITORY}/releases/download/v{PREVIOUS}/ah-windows-x64.zip"
    archive = root / "previous.zip"
    with urllib.request.urlopen(url, timeout=90) as response, archive.open("wb") as output:
        while chunk := response.read(1024 * 1024):
            output.write(chunk)
    assert archive.stat().st_size == asset["size"]
    install = root / "install"
    install.mkdir()
    executable, helper = extract_archive(archive, install)
    environment = os.environ.copy()
    environment["AH_CONFIG_DIR"] = str(root / "config")
    environment["NO_COLOR"] = "1"
    assert run(executable, ["--version"], environment) == f"ah {PREVIOUS}"
    smoke_helper(helper, PREVIOUS, environment, root)
    check = json.loads(run(executable, ["upgrade", "--check", "--json"], environment))
    assert check["current_version"] == PREVIOUS and check["selected_version"] == CURRENT, check
    print("Previous signed release installed; published update selected", flush=True)
    launch = json.loads(run(executable, ["upgrade", "--json"], environment))
    assert launch["status"] == "activation_launched", launch
    installation_id = wait_installed(executable, state, CURRENT, environment)
    backup = state / "backup" / installation_id
    assert backup.is_dir(), "Successful update must preserve its rollback backup"
    print(f"Installed version verified: {CURRENT}", flush=True)
    rollback = json.loads(run(executable, ["upgrade", "--rollback", "--json"], environment))
    assert rollback["status"] == "activation_launched", rollback
    assert wait_installed(executable, state, PREVIOUS, environment) == installation_id
    assert not backup.exists(), "Successful rollback must consume its backup"
    print(json.dumps({
        "previous_version": PREVIOUS, "upgraded_version": CURRENT,
        "rollback_version": PREVIOUS, "managed_mcp": "not_previously_running",
        "interruption": "not_tested", "reboot": "not_tested", "success": True,
    }), flush=True)


if __name__ == "__main__":
    main()
