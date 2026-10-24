#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Materialize and execute fixtures; never evaluate an optimization verdict."""
import argparse
import collections
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parent
LANGUAGES = ("rust", "c", "cpp", "zig", "go", "javascript")
NATIVE = ("rust", "c", "cpp", "zig")
EXTENSIONS = dict(zip(LANGUAGES, ("rs", "c", "cpp", "zig", "go", "mjs")))
TOOLS = {"rust": ["rustc"], "c": ["cc"], "cpp": ["c++"], "zig": ["zig"],
         "go": ["go"], "javascript": ["node"]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("prepare", "check", "measure"))
    parser.add_argument("language", choices=LANGUAGES)
    parser.add_argument("--variant", choices=("baseline", "candidate"), default="baseline")
    parser.add_argument("--case", choices=("bulk", "protected"), default="bulk")
    parser.add_argument("--out", type=Path, required=True, help="new directory outside the fixture tree")
    parser.add_argument("--route", choices=("callgrind", "gungraun"), default="callgrind")
    parser.add_argument("--allow-unsupported-host", action="store_true",
                        help="allow exploratory Go/Node measurements outside Linux x86_64")
    args = parser.parse_args()
    out = args.out.resolve()
    if out == ROOT or ROOT in out.parents:
        parser.error("output must be outside the fixture tree")
    out.mkdir(parents=True, exist_ok=False)
    project = out / "project"
    project.mkdir()
    env = {"PATH": os.environ.get("PATH", ""), "HOME": str(out / "home"),
           "LANG": "C", "LC_ALL": "C", "GOWORK": "off", "GOTOOLCHAIN": "local",
           "GOCACHE": str(out / "go-cache"), "GOMAXPROCS": "1",
           "CARGO_TARGET_DIR": str(out / "cargo-target")}
    # Rustup proxies need the installed toolchain location, not the user's credentials.
    env["RUSTUP_HOME"] = os.environ.get("RUSTUP_HOME", str(Path.home() / ".rustup"))
    Path(env["HOME"]).mkdir()
    supported = platform.system() == "Linux" and platform.machine() in ("x86_64", "AMD64")
    result = {"schema_version": 1, "language": args.language, "variant": args.variant,
              "case": args.case, "action": args.action, "status": "running",
              "host": {"system": platform.system(), "machine": platform.machine()},
              "linux_x86_64": supported, "commands": [], "source_sha256": {},
              "verdict": "not_evaluated", "profile_artifacts": [],
              "native_profiler_acceptance": False}

    def run(argv, cwd=project, timeout=180, extra_env=None, accepted_exit_codes=(0,)):
        argv = [str(value) for value in argv]
        index = len(result["commands"])
        record = {"argv": argv, "cwd": str(cwd), "timeout_seconds": timeout}
        result["commands"].append(record)
        process = subprocess.run(argv, cwd=cwd, env={**env, **(extra_env or {})},
                                 capture_output=True, text=True, timeout=timeout)
        for stream in ("stdout", "stderr"):
            name = f"command-{index:02d}.{stream}.txt"
            (out / name).write_text(getattr(process, stream))
            record[stream] = name
        record["exit_code"] = process.returncode
        if process.returncode not in accepted_exit_codes:
            raise RuntimeError(f"command {index} failed ({process.returncode}): {argv[0]}")
        return process.stdout, process.stderr

    try:
        ext = EXTENSIONS[args.language]
        for source in sorted((ROOT / args.language).iterdir()):
            if source.name in (f"baseline.{ext}", f"candidate.{ext}"):
                continue
            shutil.copy2(source, project / source.name)
        shutil.copy2(ROOT / args.language / f"{args.variant}.{ext}", project / f"kernel.{ext}")
        for source in sorted(project.iterdir()):
            result["source_sha256"][source.name] = hashlib.sha256(source.read_bytes()).hexdigest()
        result["driver_sha256"] = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
        result["manifest_sha256"] = hashlib.sha256((ROOT / "manifest.json").read_bytes()).hexdigest()
        result["source_revision"] = subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
        if args.action == "prepare":
            result["status"] = "prepared"
            return 0
        if args.action == "measure" and not supported:
            if args.language in NATIVE or not args.allow_unsupported_host:
                raise RuntimeError("unsupported platform: profiling requires Linux x86_64; "
                                   "only Go/Node exploration can override this gate")
        requested_tools = list(TOOLS[args.language])
        if args.action == "measure" and args.language in NATIVE:
            requested_tools += ["valgrind", "callgrind_annotate"]
            if args.route == "gungraun":
                requested_tools += ["cargo", "gungraun-runner"]
        for tool in requested_tools:
            if not shutil.which(tool, path=env["PATH"]):
                raise RuntimeError(f"missing tool: {tool}")
            version = "version" if tool in ("go", "zig") else "--version"
            # Valgrind's Perl annotator prints --version via die(), exiting 255.
            # Accept that status only for this probe and require its version text;
            # profile annotation must still exit successfully. Remove this exception
            # when supported Valgrind versions return zero for the version probe.
            version_codes = (0, 255) if tool == "callgrind_annotate" else (0,)
            stdout, stderr = run([tool, version], accepted_exit_codes=version_codes)
            if tool == "callgrind_annotate" and not (stdout + stderr).strip().startswith("callgrind_annotate-"):
                raise RuntimeError("callgrind_annotate returned no recognizable version")
            result.setdefault("tool_versions", {})[tool] = (stdout + stderr).strip()
        if args.language == "javascript" and not result["tool_versions"]["node"].startswith("v24."):
            raise RuntimeError("Node 24 is required for the declared runtime")
        if args.language == "zig" and result["tool_versions"]["zig"] != "0.15.2":
            raise RuntimeError("Zig 0.15.2 is required for this fixture")
        if "gungraun-runner" in requested_tools and "0.19.4" not in result["tool_versions"]["gungraun-runner"]:
            raise RuntimeError("gungraun-runner 0.19.4 must match the harness dependency")
        language = args.language
        binary = project / "workload"
        if language == "rust":
            flags = ["--edition=2021", "-C", "opt-level=2", "-C", "debuginfo=2"]
            run(["rustc", *flags, "--test", "correctness.rs", "-o", "correctness"])
            run([project / "correctness"])
            run(["rustc", *flags, "main.rs", "-o", binary])
        elif language in ("c", "cpp"):
            compiler = "cc" if language == "c" else "c++"
            flags = ["-std=c11" if language == "c" else "-std=c++17", "-O2", "-g", "-Wall", "-Wextra", "-Werror"]
            run([compiler, *flags, "-c", f"kernel.{ext}", "-o", "kernel.o"])
            run([compiler, *flags, f"correctness.{ext}", "kernel.o", "-o", "correctness"])
            run([project / "correctness"])
            run([compiler, *flags, f"main.{ext}", "kernel.o", "-o", binary])
        elif language == "zig":
            run(["zig", "test", "correctness.zig", "-O", "ReleaseSafe"])
            run(["zig", "build-exe", "main.zig", "-O", "ReleaseSafe", f"-femit-bin={binary}"])
        elif language == "go":
            run(["go", "test", "-count=1", "-run", "^Test", "./..."])
        else:
            run(["node", "--test", "correctness.test.mjs"])
        result["correctness"] = "passed"
        size = 1024 if args.case == "bulk" else 128
        values = [(i * 17 + i // 7) % 256 for i in range(size)]
        counts = collections.Counter(values)
        expected = sum(n * (n - 1) // 2 for n in counts.values())
        if language in NATIVE:
            stdout, stderr = run([binary, size])
            checksum = int((stdout + stderr).strip())
            if checksum != expected * 100:
                raise RuntimeError("workload checksum mismatch")
            result["checksum"] = checksum
        elif language == "javascript":
            stdout, _ = run(["node", "benchmark.mjs", size])
            timing = json.loads(stdout)
            if timing["checksum"] != expected * 4000:
                raise RuntimeError("workload checksum mismatch")
            result["smoke_timing"] = timing
        else:
            run(["go", "test", "-run", "^$", "-bench", f"^BenchmarkPairs$/{args.case}$", "-benchtime=1x", "-count=1"])
        if args.action == "check":
            result["status"] = "checked"
            return 0
        if language in NATIVE:
            if args.route == "callgrind":
                profile = out / "callgrind.out"
                run(["valgrind", "--tool=callgrind", "--error-exitcode=97",
                     f"--callgrind-out-file={profile}", binary, size])
                profiles = [profile]
            else:
                harness = out / "gungraun"
                (harness / "benches").mkdir(parents=True)
                shutil.copy2(ROOT / "gungraun" / "Cargo.toml.template", harness / "Cargo.toml")
                shutil.copy2(ROOT / "gungraun" / "pairs.rs", harness / "benches" / "pairs.rs")
                shutil.copy2(ROOT / "gungraun" / "Cargo.lock.template", harness / "Cargo.lock")
                run(["cargo", "bench", "--locked", "--bench", "pairs"], cwd=harness, timeout=600,
                    extra_env={"PERFINE_FIXTURE_BINARY": str(binary), "PERFINE_FIXTURE_SIZE": str(size)})
                profiles = [p for p in (out / "cargo-target").rglob("*")
                            if p.is_file() and "callgrind" in p.name and ".out" in p.name]
                if not profiles:
                    raise RuntimeError("Gungraun produced no Callgrind artifact")
            for profile in profiles:
                stdout, _ = run(["callgrind_annotate", "--inclusive=yes", profile])
                if "count_pairs" not in stdout:
                    raise RuntimeError("profile does not attribute the optimized count_pairs path")
                result["profile_artifacts"].append(str(profile.relative_to(out)))
            result["native_profiler_acceptance"] = True
        elif language == "go":
            # Profile and timing runs are separate: sampling overhead is not timing evidence.
            run(["go", "test", "-run", "^$", "-bench", f"^BenchmarkPairs$/{args.case}$",
                 "-benchtime=1s", "-count=3"])
            profile = out / "cpu.pprof"
            run(["go", "test", "-run", "^$", "-bench", f"^BenchmarkPairs$/{args.case}$",
                 "-benchtime=1s", "-count=1", f"-cpuprofile={profile}", "-o", "pairs.test"])
            stdout, _ = run(["go", "tool", "pprof", "-top", "-nodecount=0", project / "pairs.test", profile])
            if "CountPairs" not in stdout:
                raise RuntimeError("pprof does not attribute CountPairs")
            result["profile_artifacts"].append("cpu.pprof")
        else:
            for _ in range(3):
                run(["node", "benchmark.mjs", size])
            run(["node", "--cpu-prof", f"--cpu-prof-dir={out}", "--cpu-prof-name=cpu.cpuprofile", "benchmark.mjs", size])
            profile = json.loads((out / "cpu.cpuprofile").read_text())
            hot_ids = {n["id"] for n in profile["nodes"] if n["callFrame"]["functionName"] == "countPairs"}
            if not hot_ids.intersection(profile.get("samples", [])):
                raise RuntimeError("Node profile has no samples in countPairs")
            result["profile_artifacts"].append("cpu.cpuprofile")
        result["status"] = "measured"
        return 0
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        result["status"] = "failed"
        result["error"] = str(error)
        return 1
    finally:
        (out / "result.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps({"status": result["status"], "result": str(out / "result.json"),
                          "error": result.get("error")}))


if __name__ == "__main__":
    sys.exit(main())
