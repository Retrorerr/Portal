// Freestanding Windows PE fixture: tests the installed Wine/Proton runtime.
__declspec(dllimport) void *GetStdHandle(unsigned long);
__declspec(dllimport) int WriteFile(void *, const void *, unsigned long, unsigned long *, void *);
__declspec(dllimport) unsigned long GetEnvironmentVariableA(const char *, char *, unsigned long);
__declspec(dllimport) void *CreateFileA(const char *, unsigned long, unsigned long, void *, unsigned long, unsigned long, void *);
__declspec(dllimport) int CloseHandle(void *);
__declspec(dllimport) __declspec(noreturn) void ExitProcess(unsigned long);
void mainCRTStartup(void) {
    static const char result[] = "{\"windows_execution\":true}\n";
    unsigned long written = 0;
    char path[2048];
    int ok = WriteFile(GetStdHandle((unsigned long)-11), result, sizeof(result)-1, &written, (void *)0);
    unsigned long length = GetEnvironmentVariableA("PORTAL_COMPAT_RESULT", path, sizeof(path));
    if (length && length < sizeof(path)) {
        void *file = CreateFileA(path, 0x40000000, 0, (void *)0, 2, 0x80, (void *)0);
        if (file != (void *)-1) {
            ok = WriteFile(file, result, sizeof(result)-1, &written, (void *)0);
            CloseHandle(file);
        } else ok = 0;
    }
    ExitProcess(ok && written == sizeof(result)-1 ? 0 : 1);
}
