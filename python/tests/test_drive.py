"""`carla-pack drive login`: it hands rclone the right `config create` call (a fake rclone
records it), read-only unless asked to write."""

from __future__ import annotations

import io
import sys
from pathlib import Path

import pytest

from carla_pack_registry.cli import main


@pytest.fixture
def fake_rclone(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    if sys.platform == "win32":
        pytest.skip("a shell script plays rclone")
    log = tmp_path / "calls.log"
    script = tmp_path / "rclone"
    script.write_text(f'#!/bin/sh\nfor a in "$@"; do printf "%s\\n" "$a"; done > "{log}"\n')
    script.chmod(0o755)
    monkeypatch.setenv("CARLA_PACK_RCLONE", str(script))
    for var in ("CARLA_PACK_RCLONE_REMOTE", "CARLA_PACK_RCLONE_SERVICE_ACCOUNT_FILE",
                "CARLA_PACK_DRIVE_CLIENT_ID", "CARLA_PACK_DRIVE_CLIENT_SECRET"):
        monkeypatch.delenv(var, raising=False)
    return log


def test_login_creates_a_read_only_remote(fake_rclone: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("CARLA_PACK_DRIVE_CLIENT_ID", "123.apps.googleusercontent.com")
    assert main(["drive", "login"]) == 0
    assert fake_rclone.read_text().splitlines() == [
        "config", "create", "carla-pack-gdrive", "drive", "scope=drive.readonly",
        "client_id=123.apps.googleusercontent.com",
    ]


def test_login_for_publishers_and_headless_machines(fake_rclone: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(sys, "stdin", io.StringIO('{"access_token":"x","refresh_token":"y"}\n'))
    assert main(["drive", "login", "--remote", "packs:", "--write", "--token-stdin"]) == 0
    assert fake_rclone.read_text().splitlines() == [
        "config", "create", "packs", "drive", "scope=drive",
        'token={"access_token":"x","refresh_token":"y"}',
    ]
    assert main(["drive", "login", "--headless"]) == 0
    assert fake_rclone.read_text().splitlines()[-1] == "config_is_local=false"


def test_login_without_rclone(tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]) -> None:
    monkeypatch.setenv("CARLA_PACK_RCLONE", str(tmp_path / "missing-rclone"))
    assert main(["drive", "login"]) == 1
    assert "rclone was not found" in capsys.readouterr().err
