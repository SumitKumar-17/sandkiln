from sandkiln.status import to_daemon_status

BODY = {
    "version": "0.0.1",
    "uptime_secs": 5,
    "auth_enabled": True,
    "jailer_enabled": False,
    "counts": {"sandboxes": 2, "held_snapshots": 1, "retired_checkpoints": 0, "pools": 1, "pool_warm_ready": 1},
    "tap_pool": {"size": 32, "free": 30},
    "sandboxes_created_total": 7,
    "rootfs_clones": {"reflink": 7, "copy": 0},
    "disk": [{"path": "/x", "total_bytes": 10, "free_bytes": 5}],
    "warnings": ["w"],
}


def test_maps_the_wire_document():
    s = to_daemon_status(BODY)
    assert (s.sandboxes, s.tap_devices_free, s.rootfs_clones_reflink) == (2, 30, 7)
    assert s.warnings == ["w"] and s.disk[0]["free_bytes"] == 5
