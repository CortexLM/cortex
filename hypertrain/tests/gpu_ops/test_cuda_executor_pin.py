"""Copies of one fact in the CUDA path (docs/adr/0011, part 3, decision on the CUDA executor).

The container program cannot import the image's hypertrain, and the fake GPU host is written
without reading the tables the executor is checked against. Each carries its own list of the names
a pin speaks about; these tests keep every copy equal to the tables of ``protocol/env_pins.py``.
"""

from __future__ import annotations

from hypertrain.gpu_ops import cuda_executor_runner as runner
from hypertrain.gpu_ops.network_qualification import SM_COUNTS
from hypertrain.protocol.env_pins import (
    ATTENTION,
    CPU_CLASSES,
    GPU_CLASSES,
    TRAINER_SWITCHES,
    class_pin,
    gpu_label,
)

from . import fake_cuda_host as fake


def table_names() -> tuple[set[str], set[str]]:
    """(names a pin sets, names a pin removes) over every row of the tables."""
    present = {ATTENTION, *(n for row in CPU_CLASSES.values() for n in row.present)}
    absent = {*TRAINER_SWITCHES, *(n for row in CPU_CLASSES.values() for n in row.absent)}
    return present, absent


def test_the_container_program_knows_exactly_the_names_of_the_tables() -> None:
    present, absent = table_names()
    assert set(runner.PIN_PRESENT) == present
    assert list(runner.PIN_ABSENT) == sorted(absent)
    assert present.isdisjoint(absent)


def test_every_pin_the_tables_can_build_has_the_shape_the_container_program_demands() -> None:
    for cpu in CPU_CLASSES:
        for gpu, row in GPU_CLASSES.items():
            for attention in row.attention:
                body = class_pin(cpu, gpu, attention)
                assert set(body["present"]) == set(runner.PIN_PRESENT)
                assert body["absent"] == list(runner.PIN_ABSENT)
                assert gpu in runner.GPU_CLASSES


def test_the_container_program_names_the_gpu_classes_as_the_tables_do() -> None:
    assert set(runner.GPU_CLASSES) == set(GPU_CLASSES)
    for name, row in GPU_CLASSES.items():
        if row.device is not None:
            assert runner.device_class(row.device) == name == gpu_label(row.device)


def test_every_gpu_class_names_a_device_the_qualification_accepts() -> None:
    measured = {gpu_label(device): counts for device, counts in SM_COUNTS.items()}
    assert {name: row.sm_counts for name, row in GPU_CLASSES.items() if row.device} == measured
    assert {name: row.device for name, row in GPU_CLASSES.items() if row.device} == {
        gpu_label(device): device for device in SM_COUNTS
    }


def test_the_fake_host_reports_exactly_the_names_of_the_tables() -> None:
    present, absent = table_names()
    assert sorted(fake.PIN_NAMES) == sorted(present | absent)
