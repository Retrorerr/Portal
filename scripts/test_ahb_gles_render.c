#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <EGL/egl.h>
#include <EGL/eglext.h>
#include <GLES3/gl32.h>
#include <GLES2/gl2ext.h>
#include <android/hardware_buffer.h>
#include <dlfcn.h>
#include <time.h>

typedef EGLClientBuffer (*fn_eglGetNativeClientBufferANDROID)(const struct AHardwareBuffer *);

int main() {
    printf("=== TESTING GLES RENDERING DIRECTLY INTO AHARDWAREBUFFER ===\n");

    EGLDisplay dpy = eglGetDisplay(EGL_DEFAULT_DISPLAY);
    eglInitialize(dpy, NULL, NULL);

    EGLint cfg_attribs[] = {
        EGL_RENDERABLE_TYPE, EGL_OPENGL_ES3_BIT,
        EGL_SURFACE_TYPE, EGL_PBUFFER_BIT,
        EGL_NONE
    };
    EGLConfig config;
    EGLint num_configs;
    eglChooseConfig(dpy, cfg_attribs, &config, 1, &num_configs);

    EGLint ctx_attribs[] = { EGL_CONTEXT_CLIENT_VERSION, 3, EGL_NONE };
    EGLContext ctx = eglCreateContext(dpy, config, EGL_NO_CONTEXT, ctx_attribs);

    EGLint pbuf_attribs[] = { EGL_WIDTH, 16, EGL_HEIGHT, 16, EGL_NONE };
    EGLSurface surf = eglCreatePbufferSurface(dpy, config, pbuf_attribs);
    eglMakeCurrent(dpy, surf, surf, ctx);

    // 1. Allocate AHB (256x256 RGBA)
    AHardwareBuffer_Desc desc = {
        .width = 256,
        .height = 256,
        .layers = 1,
        .format = AHARDWAREBUFFER_FORMAT_R8G8B8A8_UNORM,
        .usage = AHARDWAREBUFFER_USAGE_GPU_COLOR_OUTPUT | AHARDWAREBUFFER_USAGE_GPU_SAMPLED_IMAGE | AHARDWAREBUFFER_USAGE_CPU_READ_OFTEN,
        .stride = 0,
        .rfu0 = 0,
        .rfu1 = 0
    };
    AHardwareBuffer *ahb = NULL;
    int ret = AHardwareBuffer_allocate(&desc, &ahb);
    if (ret != 0 || !ahb) {
        printf("FAILED: AHardwareBuffer_allocate returned %d\n", ret);
        return 1;
    }
    printf("1. AHardwareBuffer allocated successfully: %p\n", ahb);

    // 2. Obtain eglGetNativeClientBufferANDROID
    fn_eglGetNativeClientBufferANDROID get_native_buf =
        (fn_eglGetNativeClientBufferANDROID)eglGetProcAddress("eglGetNativeClientBufferANDROID");
    if (!get_native_buf) {
        printf("FAILED: eglGetNativeClientBufferANDROID not found\n");
        return 1;
    }
    EGLClientBuffer client_buf = get_native_buf(ahb);
    printf("2. EGLClientBuffer created: %p\n", client_buf);

    // 3. Create EGLImageKHR
    PFNEGLCREATEIMAGEKHRPROC fn_create_image = (PFNEGLCREATEIMAGEKHRPROC)eglGetProcAddress("eglCreateImageKHR");
    PFNEGLDESTROYIMAGEKHRPROC fn_destroy_image = (PFNEGLDESTROYIMAGEKHRPROC)eglGetProcAddress("eglDestroyImageKHR");
    PFNGLEGLIMAGETARGETTEXTURE2DOESPROC fn_image_target_tex =
        (PFNGLEGLIMAGETARGETTEXTURE2DOESPROC)eglGetProcAddress("glEGLImageTargetTexture2DOES");

    EGLint img_attribs[] = {
        EGL_IMAGE_PRESERVED_KHR, EGL_TRUE,
        EGL_NONE
    };
    EGLImageKHR image = fn_create_image(dpy, EGL_NO_CONTEXT, EGL_NATIVE_BUFFER_ANDROID, client_buf, img_attribs);
    if (image == EGL_NO_IMAGE_KHR) {
        printf("FAILED: eglCreateImageKHR failed with EGL error 0x%x\n", eglGetError());
        return 1;
    }
    printf("3. EGLImageKHR created: %p\n", image);

    // 4. Bind to GL texture and attach to FBO
    GLuint tex;
    glGenTextures(1, &tex);
    glBindTexture(GL_TEXTURE_2D, tex);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
    fn_image_target_tex(GL_TEXTURE_2D, image);
    GLenum gl_err = glGetError();
    if (gl_err != GL_NO_ERROR) {
        printf("FAILED: glEGLImageTargetTexture2DOES returned GL error 0x%x\n", gl_err);
        return 1;
    }
    printf("4. glEGLImageTargetTexture2DOES succeeded\n");

    GLuint fbo;
    glGenFramebuffers(1, &fbo);
    glBindFramebuffer(GL_FRAMEBUFFER, fbo);
    glFramebufferTexture2D(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_TEXTURE_2D, tex, 0);

    GLenum status = glCheckFramebufferStatus(GL_FRAMEBUFFER);
    if (status != GL_FRAMEBUFFER_COMPLETE) {
        printf("FAILED: Framebuffer not complete: 0x%x\n", status);
        return 1;
    }
    printf("5. Framebuffer complete (GL_FRAMEBUFFER_COMPLETE)!\n");

    // 5. Render directly to the AHB via GLES
    glViewport(0, 0, 256, 256);
    glClearColor(0.2f, 0.8f, 0.3f, 1.0f); // Vibrant green
    glClear(GL_COLOR_BUFFER_BIT);
    glFinish();
    printf("6. GPU glClear rendered directly into AHB!\n");

    // 6. Lock AHB to verify pixels without GL readback!
    uint8_t *pixel_ptr = NULL;
    ret = AHardwareBuffer_lock(ahb, AHARDWAREBUFFER_USAGE_CPU_READ_OFTEN, -1, NULL, (void**)&pixel_ptr);
    if (ret == 0 && pixel_ptr) {
        // Check pixel at (0, 0)
        printf("7. Pixel at (0,0): R=%u G=%u B=%u A=%u\n",
            pixel_ptr[0], pixel_ptr[1], pixel_ptr[2], pixel_ptr[3]);
        // Expected ~51, 204, 76, 255 (RGBA for 0.2, 0.8, 0.3, 1.0)
        if (pixel_ptr[1] > 180 && pixel_ptr[0] < 70 && pixel_ptr[2] < 90) {
            printf(">>> SUCCESS: ZERO-COPY HARDWARE RENDERING INTO AHARDWAREBUFFER VERIFIED! <<<\n");
        } else {
            printf("WARNING: Pixel value did not match expected clear color.\n");
        }
        AHardwareBuffer_unlock(ahb, NULL);
    } else {
        printf("FAILED: AHardwareBuffer_lock returned %d\n", ret);
    }

    // Cleanup
    glDeleteFramebuffers(1, &fbo);
    glDeleteTextures(1, &tex);
    fn_destroy_image(dpy, image);
    AHardwareBuffer_release(ahb);

    eglMakeCurrent(dpy, EGL_NO_SURFACE, EGL_NO_SURFACE, EGL_NO_CONTEXT);
    eglDestroySurface(dpy, surf);
    eglDestroyContext(dpy, ctx);
    eglTerminate(dpy);

    return 0;
}
