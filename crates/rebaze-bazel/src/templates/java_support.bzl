"""The native Gradle generator supports JVM JAR dependencies only."""

def aar_import(**_attrs):
    fail("Android AAR dependencies are unsupported by rebaze's native Java generator")
