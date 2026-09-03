#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <sys/socket.h>
#include <sys/wait.h>
#include <EGL/egl.h>
#include <EGL/eglext.h>
#include <GLES3/gl32.h>
#include <GLES2/gl2ext.h>
#include <android/hardware_buffer.h>

typedef EGLClientBuffer (*fn_eglGetNativeClientBufferANDROID)(const struct AHardwareBuffer *);

int main() {
    printf("=== TESTING AHARDWAREBUFFER CROSS-PROCESS UNIX SOCKET IPC ===\n");

    int sv[2];
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, sv) != 0) {
        perror("socketpair");
        return 1;
    }

    pid_t pid = fork();
    if (pid == 0) {
        // Child: Client that allocates AHB, renders purple, and sends to parent
        close(sv[0]);

        EGLDisplay dpy = eglGetDisplay(EGL_DEFAULT_DISPLAY);
        eglInitialize(dpy, NULL, NULL);
        EGLint cfg_attribs[] = { EGL_RENDERABLE_TYPE, EGL_OPENGL_ES3_BIT, EGL_SURFACE_TYPE, EGL_PBUFFER_BIT, EGL_NONE };
        EGLConfig config;
        EGLint num_configs;
        eglChooseConfig(dpy, cfg_attribs, &config, 1, &num_configs);
        EGLint ctx_attribs[] = { EGL_CONTEXT_CLIENT_VERSION, 3, EGL_NONE };
        EGLContext ctx = eglCreateContext(dpy, config, EGL_NO_CONTEXT, ctx_attribs);
        EGLint pbuf_attribs[] = { EGL_WIDTH, 16, EGL_HEIGHT, 16, EGL_NONE };
        EGLSurface surf = eglCreatePbufferSurface(dpy, config, pbuf_attribs);
        eglMakeCurrent(dpy, surf, surf, ctx);

        AHardwareBuffer_Desc desc = {
            .width = 128,
            .height = 128,
            .layers = 1,
            .format = AHARDWAREBUFFER_FORMAT_R8G8B8A8_UNORM,
            .usage = AHARDWAREBUFFER_USAGE_GPU_COLOR_OUTPUT | AHARDWAREBUFFER_USAGE_GPU_SAMPLED_IMAGE,
            .stride = 0,
            .rfu0 = 0,
            .rfu1 = 0
        };
        AHardwareBuffer *ahb = NULL;
        AHardwareBuffer_allocate(&desc, &ahb);

        fn_eglGetNativeClientBufferANDROID get_native_buf =
            (fn_eglGetNativeClientBufferANDROID)eglGetProcAddress("eglGetNativeClientBufferANDROID");
        EGLClientBuffer client_buf = get_native_buf(ahb);
        PFNEGLCREATEIMAGEKHRPROC fn_create_image = (PFNEGLCREATEIMAGEKHRPROC)eglGetProcAddress("eglCreateImageKHR");
        PFNGLEGLIMAGETARGETTEXTURE2DOESPROC fn_image_target_tex =
            (PFNGLEGLIMAGETARGETTEXTURE2DOESPROC)eglGetProcAddress("glEGLImageTargetTexture2DOES");

        EGLImageKHR img = fn_create_image(dpy, EGL_NO_CONTEXT, EGL_NATIVE_BUFFER_ANDROID, client_buf, NULL);
        GLuint tex;
        glGenTextures(1, &tex);
        glBindTexture(GL_TEXTURE_2D, tex);
        fn_image_target_tex(GL_TEXTURE_2D, img);

        GLuint fbo;
        glGenFramebuffers(1, &fbo);
        glBindFramebuffer(GL_FRAMEBUFFER, fbo);
        glFramebufferTexture2D(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_TEXTURE_2D, tex, 0);

        glViewport(0, 0, 128, 128);
        glClearColor(0.8f, 0.1f, 0.9f, 1.0f); // Purple
        glClear(GL_COLOR_BUFFER_BIT);
        glFinish();

        // Send AHB handle across unix socket to parent
        int res = AHardwareBuffer_sendHandleToUnixSocket(ahb, sv[1]);
        printf("[Child] AHardwareBuffer_sendHandleToUnixSocket: res=%d\n", res);

        AHardwareBuffer_release(ahb);
        close(sv[1]);
        _exit(0);
    } else {
        // Parent: Compositor side that receives AHB and inspects/samples it
        close(sv[1]);

        AHardwareBuffer *recv_ahb = NULL;
        int res = AHardwareBuffer_recvHandleFromUnixSocket(sv[0], &recv_ahb);
        printf("[Parent] AHardwareBuffer_recvHandleFromUnixSocket: res=%d ahb=%p\n", res, recv_ahb);

        if (res == 0 && recv_ahb) {
            AHardwareBuffer_Desc desc;
            AHardwareBuffer_describe(recv_ahb, &desc);
            printf("[Parent] Received AHB desc: width=%u height=%u format=%u stride=%u\n",
                desc.width, desc.height, desc.format, desc.stride);

            // Verify content by locking
            uint8_t *pixels = NULL;
            res = AHardwareBuffer_lock(recv_ahb, AHARDWAREBUFFER_USAGE_CPU_READ_OFTEN, -1, NULL, (void**)&pixels);
            if (res == 0 && pixels) {
                printf("[Parent] Pixel at (0,0): R=%u G=%u B=%u A=%u\n",
                    pixels[0], pixels[1], pixels[2], pixels[3]);
                if (pixels[0] > 180 && pixels[2] > 200 && pixels[1] < 50) {
                    printf(">>> CROSS-PROCESS AHB ZERO-COPY TRANSPORT VERIFIED! <<<\n");
                }
                AHardwareBuffer_unlock(recv_ahb, NULL);
            }
            AHardwareBuffer_release(recv_ahb);
        }

        close(sv[0]);
        waitpid(pid, NULL, 0);
    }

    return 0;
}
