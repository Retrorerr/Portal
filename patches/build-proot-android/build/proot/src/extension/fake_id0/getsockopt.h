#ifndef FAKE_ID0_GETSOCKOPT_H
#define FAKE_ID0_GETSOCKOPT_H

#include "tracee/tracee.h"
#include "extension/fake_id0/config.h"

int handle_getsockopt_enter_end(Tracee *tracee, Config *config);
int handle_getsockopt_exit_end(Tracee *tracee, Config *config);

#endif /* FAKE_ID0_GETSOCKOPT_H */
