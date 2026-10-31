#!/usr/bin/env python3
"""Test virtual controller pointer ownership with the cached Android Kotlin compiler."""
from pathlib import Path
import argparse
import os
import shlex
import shutil
import subprocess
import tempfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--serial", help="Also test the installed Portal Debug controller views on this adb target")
args = parser.parse_args()
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
with tempfile.TemporaryDirectory(prefix="portal-controller-") as output:
    subprocess.run([
        "java", "-cp", classpath, "org.jetbrains.kotlin.cli.jvm.K2JVMCompiler",
        "-no-stdlib", "-no-reflect", "-classpath", stdlib, "-d", output,
        str(root / "src/android/kotlin/app/polarbear/gaming/ControllerPointers.kt"),
        str(root / "tests/kotlin/ControllerPointersTest.kt"),
    ], check=True)
    subprocess.run([
        "java", "-cp", output + os.pathsep + stdlib,
        "app.polarbear.gaming.ControllerPointersTestKt",
    ], check=True)

# Optional Android regression: the actual ControlView must publish neutral state,
# not merely redraw its knob at the centre. This uses a recording ControlHost.
if args.serial:
    sdk = Path(os.environ.get("ANDROID_HOME", os.environ.get("ANDROID_SDK_ROOT", Path.home() / "Android/Sdk")))
    platform = sdk / "platforms/android-35/android.jar"
    javac = shutil.which("javac")
    if not javac and os.environ.get("JAVA_HOME"):
        javac = str(Path(os.environ["JAVA_HOME"]) / "bin/javac")
    if not javac or not Path(javac).is_file():
        raise SystemExit("Set JAVA_HOME to a JDK or put javac on PATH for the Android view regression")
    versions = sorted((sdk / "build-tools").glob("*/d8"), key=lambda p: tuple(int(v) for v in p.parent.name.split(".") if v.isdigit()))
    if not versions or not platform.is_file():
        raise SystemExit("Android SDK platform 35 and build-tools with d8 are required")
    adb = ["adb", "-s", args.serial]
    with tempfile.TemporaryDirectory(prefix="portal-controller-view-") as output:
        output = Path(output)
        subprocess.run([javac, "-classpath", str(platform), "-d", str(output),
                        str(root / "tests/android/ControllerInputProbe.java")], check=True)
        archive = output / "controller-probe.jar"
        subprocess.run([str(versions[-1]), "--min-api", "29", "--output", str(archive),
                        str(output / "ControllerInputProbe.class")], check=True)
        remote = "/data/local/tmp/" + output.name + ".jar"
        try:
            subprocess.run(adb + ["push", str(archive), remote], check=True)
            subprocess.run(adb + ["shell", "CLASSPATH=" + shlex.quote(remote) +
                                " app_process /system/bin ControllerInputProbe"], check=True)
        finally:
            subprocess.run(adb + ["shell", "rm", "-f", remote], check=False)
