#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "dep.h"

int scope_value(void);

int main(int argc, char **argv) {
    int value = dep_value() + scope_value();
    if (argc != 4 || strcmp(argv[1], "two words'$") != 0 || atoi(argv[2]) != value || argv[3][0] != '\0') {
        fprintf(stderr, "fixture assertion failed\n");
        return 1;
    }
    printf("rebaze fixture: %d\n", value);
    return 0;
}
