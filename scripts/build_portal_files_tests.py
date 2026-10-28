#!/usr/bin/env python3
"""Build a small standalone instrumentation APK targeting Portal Debug only."""
from pathlib import Path
import os
import shutil
import subprocess
import zipfile

root = Path(__file__).resolve().parents[1]
sdk = Path(os.environ.get("ANDROID_HOME", str(Path.home() / "Android/Sdk")))
tools = sdk / "build-tools/35.0.0"
android = sdk / "platforms/android-35/android.jar"
output = root / "target/portal-files-tests"
classes = output / "classes"
dex = output / "dex"
classes.mkdir(parents=True, exist_ok=True)
dex.mkdir(parents=True, exist_ok=True)
javac = shutil.which("javac") or str(Path(shutil.which("java")).resolve().with_name("javac"))
subprocess.run([javac, "-source", "8", "-target", "8", "-classpath", str(android), "-d", str(classes),
                *map(str, (root / "tests/android").glob("*.java"))], check=True)
jar = output / "tests.jar"
with zipfile.ZipFile(jar, "w") as archive:
    for file in classes.rglob("*.class"):
        archive.write(file, file.relative_to(classes))
subprocess.run([str(tools / "d8"), "--min-api", "24", "--lib", str(android), "--output", str(dex), str(jar)], check=True)
unsigned = output / "unsigned.apk"
subprocess.run([str(tools / "aapt2"), "link", "-I", str(android), "--manifest", str(root / "tests/android/AndroidManifest.xml"), "-o", str(unsigned)], check=True)
with zipfile.ZipFile(unsigned, "a") as archive:
    archive.write(dex / "classes.dex", "classes.dex")
aligned = output / "aligned.apk"
subprocess.run([str(tools / "zipalign"), "-f", "4", str(unsigned), str(aligned)], check=True)
apk = output / "Portal-Files-Tests.apk"
subprocess.run([str(tools / "apksigner"), "sign", "--ks", str(Path.home() / ".android/debug.keystore"),
                "--ks-pass", "pass:android", "--key-pass", "pass:android", "--out", str(apk), str(aligned)], check=True)
print(apk)
