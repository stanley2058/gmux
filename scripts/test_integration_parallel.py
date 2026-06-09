from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
TESTS_DIR = ROOT / "tests"


@dataclass(frozen=True)
class TestBinary:
    name: str
    executable: Path


@dataclass(frozen=True)
class TestResult:
    binary: TestBinary
    returncode: int
    elapsed: float
    stdout: str
    stderr: str


def discover_test_binaries(test_names: set[str]) -> list[TestBinary]:
    command = [
        "cargo",
        "test",
        "--locked",
        "--tests",
        "--no-run",
        "--message-format=json",
    ]
    result = subprocess.run(
        command,
        cwd=ROOT,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if result.returncode != 0:
        sys.stderr.write(result.stderr)
        raise SystemExit(result.returncode)

    binaries: dict[str, TestBinary] = {}
    for line in result.stdout.splitlines():
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue

        if message.get("reason") != "compiler-artifact":
            continue

        target = message.get("target", {})
        name = target.get("name")
        executable = message.get("executable")
        if name not in test_names or executable is None:
            continue

        src_path = Path(target.get("src_path", ""))
        if not src_path.is_absolute():
            src_path = ROOT / src_path
        if src_path.parent != TESTS_DIR:
            continue

        binaries[name] = TestBinary(name=name, executable=Path(executable))

    missing = sorted(test_names - set(binaries))
    if missing:
        missing_list = ", ".join(missing)
        raise SystemExit(f"missing integration test binaries: {missing_list}")

    return [binaries[name] for name in sorted(binaries)]


def run_binary(binary: TestBinary, test_threads: int) -> TestResult:
    command = [
        str(binary.executable),
        f"--test-threads={test_threads}",
        "--format",
        "terse",
    ]
    started = time.monotonic()
    result = subprocess.run(
        command,
        cwd=ROOT,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    elapsed = time.monotonic() - started
    return TestResult(
        binary=binary,
        returncode=result.returncode,
        elapsed=elapsed,
        stdout=result.stdout,
        stderr=result.stderr,
    )


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Build and run gmux integration test binaries concurrently."
    )
    parser.add_argument(
        "--jobs",
        type=int,
        default=int(os.environ.get("GMUX_TEST_INTEGRATION_JOBS", "0")),
        help="number of integration test binaries to run at once",
    )
    parser.add_argument(
        "--test-threads",
        type=int,
        default=int(os.environ.get("GMUX_TEST_THREADS", "1")),
        help="libtest threads per integration test binary",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    test_names = {path.stem for path in TESTS_DIR.glob("*.rs") if path.name != "support.rs"}
    if not test_names:
        raise SystemExit("no integration tests found")

    binaries = discover_test_binaries(test_names)
    jobs = args.jobs if args.jobs > 0 else min(len(binaries), os.cpu_count() or 1)
    if jobs <= 0:
        raise SystemExit("--jobs must be greater than zero")
    if args.test_threads <= 0:
        raise SystemExit("--test-threads must be greater than zero")

    print(
        f"running {len(binaries)} integration test binaries with {jobs} jobs "
        f"and {args.test_threads} test thread(s) each"
    )
    started = time.monotonic()
    failures: list[TestResult] = []
    with ThreadPoolExecutor(max_workers=jobs) as executor:
        future_results = {
            executor.submit(run_binary, binary, args.test_threads): binary
            for binary in binaries
        }
        for future in as_completed(future_results):
            result = future.result()
            status = "ok" if result.returncode == 0 else "FAILED"
            print(f"{status:6} {result.binary.name} ({result.elapsed:.2f}s)")
            if result.returncode != 0:
                failures.append(result)

    elapsed = time.monotonic() - started
    if not failures:
        print(f"integration tests passed in {elapsed:.2f}s")
        return 0

    print(f"\n{len(failures)} integration test binary failure(s):", file=sys.stderr)
    for failure in failures:
        print(f"\n===== {failure.binary.name} stdout =====", file=sys.stderr)
        sys.stderr.write(failure.stdout)
        print(f"\n===== {failure.binary.name} stderr =====", file=sys.stderr)
        sys.stderr.write(failure.stderr)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
