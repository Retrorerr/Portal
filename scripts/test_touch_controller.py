#!/usr/bin/env python3
"""Test the on-screen controller's layout and touch feel with the cached Android Kotlin compiler."""
from pathlib import Path
import argparse
import os
import shlex
import shutil
import subprocess
import tempfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--serial", help="Also test the installed Portal Debug controller on this adb target")
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
        *(str(root / "src/android/kotlin/app/polarbear/gaming" / name) for name in ("Control.kt", "PadLayout.kt", "PadInput.kt")),
        str(root / "tests/kotlin/GamepadTest.kt"),
    ], check=True)
    subprocess.run([
        "java", "-cp", output + os.pathsep + stdlib,
        "app.polarbear.gaming.GamepadTestKt",
    ], check=True)

# Optional Android regression: the installed Gamepad, floating and whole-screen,
# must publish neutral state on every way a touch ends and pass a finger off
# every control to the desktop. This uses a recording PadSink.
if args.serial:
    home_sdk = Path(os.environ["LOCALAPPDATA"]) / "Android/Sdk" if os.name == "nt" else Path.home() / "Android/Sdk"
    sdk = Path(os.environ.get("ANDROID_HOME", os.environ.get("ANDROID_SDK_ROOT", home_sdk)))
    platform = sdk / "platforms/android-35/android.jar"
    javac = shutil.which("javac")
    if not javac and os.environ.get("JAVA_HOME"):
        javac = str(Path(os.environ["JAVA_HOME"]) / "bin/javac")
    if not javac or not Path(javac).is_file():
        raise SystemExit("Set JAVA_HOME to a JDK or put javac on PATH for the Android view regression")
    versions = sorted((sdk / "build-tools").glob("*/d8.bat" if os.name == "nt" else "*/d8"), key=lambda p: tuple(int(v) for v in p.parent.name.split(".") if v.isdigit()))
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
