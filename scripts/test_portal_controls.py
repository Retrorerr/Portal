#!/usr/bin/env python3
"""Run Portal's pure Kotlin capsule geometry tests with the Android build's cached compiler."""
from pathlib import Path
import os
import subprocess
import tempfile

root = Path(__file__).resolve().parents[1]
cache = Path.home() / ".gradle/caches/modules-2/files-2.1"


def jar(group, name, version="*"):
    matches = sorted((cache / group / name).glob(f"{version}/*/{name}-*.jar"))
    if not matches:
        raise SystemExit(f"Missing cached {name}; run the Android build first")
    return str(matches[-1])


stdlib = jar("org.jetbrains.kotlin", "kotlin-stdlib", "2.0.21")
classpath = os.pathsep.join([
    jar("org.jetbrains.kotlin", "kotlin-compiler-embeddable", "2.0.21"), stdlib,
    jar("org.jetbrains.kotlin", "kotlin-script-runtime", "2.0.21"),
    jar("org.jetbrains.kotlin", "kotlin-reflect"),
    jar("org.jetbrains.intellij.deps", "trove4j"),
    jar("org.jetbrains", "annotations"),
    jar("org.jetbrains.kotlinx", "kotlinx-coroutines-core-jvm"),
])
with tempfile.TemporaryDirectory(prefix="portal-controls-") as output:
    subprocess.run([
        "java", "-cp", classpath, "org.jetbrains.kotlin.cli.jvm.K2JVMCompiler",
        "-no-stdlib", "-no-reflect", "-classpath", stdlib, "-d", output,
        str(root / "src/android/kotlin/app/polarbear/CapsulePosition.kt"),
        str(root / "src/android/kotlin/app/polarbear/UtilityMenuGeometry.kt"),
        str(root / "tests/kotlin/CapsulePositionTest.kt"),
    ], check=True)
    subprocess.run(["java", "-cp", output + os.pathsep + stdlib, "app.polarbear.CapsulePositionTestKt"], check=True)
