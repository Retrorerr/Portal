#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <sys/socket.h>
#include <android/hardware_buffer.h>

int main() {
    int sv[2];
    socketpair(AF_UNIX, SOCK_STREAM, 0, sv);

    AHardwareBuffer_Desc desc = {
        .width = 64,
        .height = 64,
        .layers = 1,
        .format = AHARDWAREBUFFER_FORMAT_R8G8B8A8_UNORM,
        .usage = AHARDWAREBUFFER_USAGE_GPU_COLOR_OUTPUT | AHARDWAREBUFFER_USAGE_GPU_SAMPLED_IMAGE,
    };
    AHardwareBuffer *ahb = NULL;
    AHardwareBuffer_allocate(&desc, &ahb);

    pid_t pid = fork();
    if (pid == 0) {
        close(sv[0]);
        AHardwareBuffer_sendHandleToUnixSocket(ahb, sv[1]);
        close(sv[1]);
        _exit(0);
    } else {
        close(sv[1]);
        char buf[1024];
        char cmsg_buf[1024];
        struct iovec iov = { .iov_base = buf, .iov_len = sizeof(buf) };
        struct msghdr msg = {
            .msg_iov = &iov,
            .msg_iovlen = 1,
            .msg_control = cmsg_buf,
            .msg_controllen = sizeof(cmsg_buf)
        };
        ssize_t n = recvmsg(sv[0], &msg, 0);
        printf("recvmsg: received %zd bytes\n", n);

        struct cmsghdr *cmsg = CMSG_FIRSTHDR(&msg);
        while (cmsg) {
            if (cmsg->cmsg_level == SOL_SOCKET && cmsg->cmsg_type == SCM_RIGHTS) {
                int num_fds = (cmsg->cmsg_len - CMSG_LEN(0)) / sizeof(int);
                printf("  Received SCM_RIGHTS with %d fds:\n", num_fds);
                int *fds = (int*)CMSG_DATA(cmsg);
                for (int i = 0; i < num_fds; i++) {
                    printf("    fd[%d] = %d\n", i, fds[i]);
                    close(fds[i]);
                }
            }
            cmsg = CMSG_NXTHDR(&msg, cmsg);
        }
        close(sv[0]);
        AHardwareBuffer_release(ahb);
    }
    return 0;
}
