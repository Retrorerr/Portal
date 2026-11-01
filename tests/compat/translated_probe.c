#include <stdio.h>
#include <string.h>
int main(int argc, char **argv) {
    if (argc != 3 || strcmp(argv[1], "a b") || strcmp(argv[2], "日本語")) return 2;
    puts("{\"translated_linux\":true,\"arguments_preserved\":true}");
    return 0;
}
