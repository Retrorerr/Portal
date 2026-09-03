#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <EGL/egl.h>
#include <EGL/eglext.h>
#include <GLES3/gl32.h>
#include <GLES2/gl2ext.h>
#include <vulkan/vulkan.h>
#include <android/hardware_buffer.h>
#include <dlfcn.h>

int main() {
    printf("=== ADRENO GPU CAPABILITIES PROBE ===\n");

    // 1. EGL & GLES Probe
    EGLDisplay dpy = eglGetDisplay(EGL_DEFAULT_DISPLAY);
    if (dpy == EGL_NO_DISPLAY) {
        printf("EGL: eglGetDisplay failed\n");
    } else {
        EGLint major, minor;
        if (eglInitialize(dpy, &major, &minor)) {
            printf("EGL: Initialized %d.%d\n", major, minor);
            const char *egl_exts = eglQueryString(dpy, EGL_EXTENSIONS);
            printf("EGL Extensions: %s\n", egl_exts ? egl_exts : "null");

            printf("Has EGL_ANDROID_get_native_client_buffer: %d\n",
                egl_exts && strstr(egl_exts, "EGL_ANDROID_get_native_client_buffer") != NULL);
            printf("Has EGL_ANDROID_image_native_buffer: %d\n",
                egl_exts && strstr(egl_exts, "EGL_ANDROID_image_native_buffer") != NULL);
            printf("Has EGL_EXT_image_dma_buf_import: %d\n",
                egl_exts && strstr(egl_exts, "EGL_EXT_image_dma_buf_import") != NULL);
            printf("Has EGL_ANDROID_native_fence_sync: %d\n",
                egl_exts && strstr(egl_exts, "EGL_ANDROID_native_fence_sync") != NULL);

            EGLint cfg_attribs[] = {
                EGL_RENDERABLE_TYPE, EGL_OPENGL_ES3_BIT,
                EGL_SURFACE_TYPE, EGL_PBUFFER_BIT,
                EGL_NONE
            };
            EGLConfig config;
            EGLint num_configs;
            eglChooseConfig(dpy, cfg_attribs, &config, 1, &num_configs);

            EGLint ctx_attribs[] = {
                EGL_CONTEXT_CLIENT_VERSION, 3,
                EGL_NONE
            };
            EGLContext ctx = eglCreateContext(dpy, config, EGL_NO_CONTEXT, ctx_attribs);
            if (ctx != EGL_NO_CONTEXT) {
                EGLint pbuf_attribs[] = { EGL_WIDTH, 16, EGL_HEIGHT, 16, EGL_NONE };
                EGLSurface surf = eglCreatePbufferSurface(dpy, config, pbuf_attribs);
                eglMakeCurrent(dpy, surf, surf, ctx);

                printf("GL_VENDOR: %s\n", glGetString(GL_VENDOR));
                printf("GL_RENDERER: %s\n", glGetString(GL_RENDERER));
                printf("GL_VERSION: %s\n", glGetString(GL_VERSION));

                const char *gl_exts = (const char *)glGetString(GL_EXTENSIONS);
                printf("Has GL_OES_EGL_image: %d\n",
                    gl_exts && strstr(gl_exts, "GL_OES_EGL_image") != NULL);
                printf("Has GL_OES_EGL_image_external: %d\n",
                    gl_exts && strstr(gl_exts, "GL_OES_EGL_image_external") != NULL);
                printf("Has GL_EXT_memory_object: %d\n",
                    gl_exts && strstr(gl_exts, "GL_EXT_memory_object") != NULL);
                printf("Has GL_EXT_memory_object_fd: %d\n",
                    gl_exts && strstr(gl_exts, "GL_EXT_memory_object_fd") != NULL);

                eglMakeCurrent(dpy, EGL_NO_SURFACE, EGL_NO_SURFACE, EGL_NO_CONTEXT);
                eglDestroySurface(dpy, surf);
                eglDestroyContext(dpy, ctx);
            }
            eglTerminate(dpy);
        }
    }

    // 2. AHardwareBuffer & Native Handle Probe
    AHardwareBuffer_Desc desc = {
        .width = 64,
        .height = 64,
        .layers = 1,
        .format = AHARDWAREBUFFER_FORMAT_R8G8B8A8_UNORM,
        .usage = AHARDWAREBUFFER_USAGE_GPU_SAMPLED_IMAGE | AHARDWAREBUFFER_USAGE_GPU_COLOR_OUTPUT | AHARDWAREBUFFER_USAGE_CPU_READ_RARELY,
        .stride = 0,
        .rfu0 = 0,
        .rfu1 = 0
    };
    AHardwareBuffer *ahb = NULL;
    int res = AHardwareBuffer_allocate(&desc, &ahb);
    printf("\nAHardwareBuffer_allocate: %d (ptr=%p)\n", res, ahb);
    if (res == 0 && ahb) {
        AHardwareBuffer_describe(ahb, &desc);
        printf("AHB describe: width=%u height=%u stride=%u format=%u\n",
            desc.width, desc.height, desc.stride, desc.format);

        void *libnw = dlopen("libnativewindow.so", RTLD_NOW | RTLD_GLOBAL);
        if (libnw) {
            typedef void* (*fn_get_handle)(const AHardwareBuffer*);
            fn_get_handle get_handle = (fn_get_handle)dlsym(libnw, "AHardwareBuffer_getNativeHandle");
            if (get_handle) {
                struct {
                    int version;
                    int numFds;
                    int numInts;
                    int data[16];
                } *nh = get_handle(ahb);
                if (nh) {
                    printf("NativeHandle: version=%d numFds=%d numInts=%d\n",
                        nh->version, nh->numFds, nh->numInts);
                    for (int i = 0; i < nh->numFds; i++) {
                        printf("  fd[%d] = %d\n", i, nh->data[i]);
                    }
                }
            }
        }
        AHardwareBuffer_release(ahb);
    }

    // 3. Vulkan Probe
    printf("\n=== VULKAN PROBE ===\n");
    VkApplicationInfo appInfo = {
        .sType = VK_STRUCTURE_TYPE_APPLICATION_INFO,
        .pApplicationName = "gpu_probe",
        .apiVersion = VK_API_VERSION_1_1
    };
    VkInstanceCreateInfo instInfo = {
        .sType = VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO,
        .pApplicationInfo = &appInfo
    };
    VkInstance instance;
    if (vkCreateInstance(&instInfo, NULL, &instance) == VK_SUCCESS) {
        uint32_t devCount = 0;
        vkEnumeratePhysicalDevices(instance, &devCount, NULL);
        printf("Vulkan physical devices: %u\n", devCount);
        if (devCount > 0) {
            VkPhysicalDevice *devs = malloc(sizeof(VkPhysicalDevice) * devCount);
            vkEnumeratePhysicalDevices(instance, &devCount, devs);
            for (uint32_t i = 0; i < devCount; i++) {
                VkPhysicalDeviceProperties props;
                vkGetPhysicalDeviceProperties(devs[i], &props);
                printf("Device %u: %s (API %u.%u.%u, Driver 0x%x)\n",
                    i, props.deviceName,
                    VK_VERSION_MAJOR(props.apiVersion),
                    VK_VERSION_MINOR(props.apiVersion),
                    VK_VERSION_PATCH(props.apiVersion),
                    props.driverVersion);

                uint32_t extCount = 0;
                vkEnumerateDeviceExtensionProperties(devs[i], NULL, &extCount, NULL);
                VkExtensionProperties *exts = malloc(sizeof(VkExtensionProperties) * extCount);
                vkEnumerateDeviceExtensionProperties(devs[i], NULL, &extCount, exts);

                int has_ahb = 0, has_ext_fd = 0, has_dma_buf = 0;
                for (uint32_t e = 0; e < extCount; e++) {
                    if (strcmp(exts[e].extensionName, "VK_ANDROID_external_memory_android_hardware_buffer") == 0) has_ahb = 1;
                    if (strcmp(exts[e].extensionName, "VK_KHR_external_memory_fd") == 0) has_ext_fd = 1;
                    if (strcmp(exts[e].extensionName, "VK_EXT_external_memory_dma_buf") == 0) has_dma_buf = 1;
                }
                printf("  Has VK_ANDROID_external_memory_android_hardware_buffer: %d\n", has_ahb);
                printf("  Has VK_KHR_external_memory_fd: %d\n", has_ext_fd);
                printf("  Has VK_EXT_external_memory_dma_buf: %d\n", has_dma_buf);
                free(exts);
            }
            free(devs);
        }
        vkDestroyInstance(instance, NULL);
    } else {
        printf("Vulkan: vkCreateInstance failed\n");
    }

    return 0;
}
