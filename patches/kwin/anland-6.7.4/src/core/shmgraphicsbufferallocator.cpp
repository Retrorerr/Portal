/*
    SPDX-FileCopyrightText: 2023 Vlad Zahorodnii <vlad.zahorodnii@kde.org>

    SPDX-License-Identifier: GPL-2.0-or-later
*/

#include "core/shmgraphicsbufferallocator.h"

#include "config-kwin.h"

#include "core/graphicsbuffer.h"
#include "utils/common.h"
#include "utils/memorymap.h"

#include <drm_fourcc.h>
#include <cerrno>
#include <cstring>
#include <fcntl.h>
#include <limits>
#include <sys/mman.h>
#include <unistd.h>

namespace KWin
{

class ShmGraphicsBuffer : public GraphicsBuffer
{
    Q_OBJECT

public:
    ShmGraphicsBuffer(ShmAttributes &&attributes, MemoryMap &&memoryMap);

    Map map(MapFlags flags) override;
    void unmap() override;

    QSize size() const override;
    bool hasAlphaChannel() const override;
    const ShmAttributes *shmAttributes() const override;

private:
    ShmAttributes m_attributes;
    MemoryMap m_memoryMap;
    bool m_hasAlphaChannel;
};

ShmGraphicsBuffer::ShmGraphicsBuffer(ShmAttributes &&attributes, MemoryMap &&memoryMap)
    : m_attributes(std::move(attributes))
    , m_memoryMap(std::move(memoryMap))
    , m_hasAlphaChannel(alphaChannelFromDrmFormat(attributes.format))
{
}

GraphicsBuffer::Map ShmGraphicsBuffer::map(MapFlags flags)
{
    if (m_memoryMap.isValid()) {
        return Map{
            .data = m_memoryMap.data(),
            .stride = uint32_t(m_attributes.stride),
        };
    } else {
        return Map{};
    }
}

void ShmGraphicsBuffer::unmap()
{
}

QSize ShmGraphicsBuffer::size() const
{
    return m_attributes.size;
}

bool ShmGraphicsBuffer::hasAlphaChannel() const
{
    return m_hasAlphaChannel;
}

const ShmAttributes *ShmGraphicsBuffer::shmAttributes() const
{
    return &m_attributes;
}

GraphicsBuffer *ShmGraphicsBufferAllocator::allocate(const GraphicsBufferOptions &options)
{
    if (!options.software) {
        return nullptr;
    }
    if (!options.modifiers.empty() && !options.modifiers.contains(DRM_FORMAT_MOD_LINEAR)) {
        return nullptr;
    }

    switch (options.format) {
    case DRM_FORMAT_ARGB8888:
    case DRM_FORMAT_XRGB8888:
        break;
    default:
        return nullptr;
    }

    if (options.size.width() <= 0 || options.size.height() <= 0) {
        qCWarning(KWIN_CORE) << "SHM allocator rejected empty size" << options.size;
        return nullptr;
    }

    const qint64 maxBufferSize = std::numeric_limits<int>::max();
    const qint64 stride64 = qint64(options.size.width()) * 4;
    if (stride64 > maxBufferSize || options.size.height() > maxBufferSize / stride64) {
        qCWarning(KWIN_CORE) << "SHM allocator rejected oversized buffer"
                             << "size" << options.size
                             << "stride" << stride64;
        return nullptr;
    }

    const qint64 bufferSize64 = qint64(options.size.height()) * stride64;
    const int stride = int(stride64);
    const int bufferSize = int(bufferSize64);

#if HAVE_MEMFD
    FileDescriptor fd = FileDescriptor(memfd_create("shm", MFD_CLOEXEC | MFD_ALLOW_SEALING));
    if (!fd.isValid()) {
        const int error = errno;
        qCWarning(KWIN_CORE) << "SHM allocator memfd_create failed"
                             << "size" << options.size
                             << "bytes" << bufferSize64
                             << "error" << strerror(error);
        return nullptr;
    }

    if (ftruncate(fd.get(), bufferSize) < 0) {
        const int error = errno;
        qCWarning(KWIN_CORE) << "SHM allocator ftruncate failed"
                             << "size" << options.size
                             << "bytes" << bufferSize64
                             << "error" << strerror(error);
        return nullptr;
    }

    fcntl(fd.get(), F_ADD_SEALS, F_SEAL_SHRINK | F_SEAL_GROW | F_SEAL_SEAL);
#else
    char templateName[] = "/tmp/kwin-shm-XXXXXX";
    FileDescriptor fd{mkstemp(templateName)};
    if (!fd.isValid()) {
        const int error = errno;
        qCWarning(KWIN_CORE) << "SHM allocator mkstemp failed"
                             << "size" << options.size
                             << "bytes" << bufferSize64
                             << "error" << strerror(error);
        return nullptr;
    }

    unlink(templateName);
    int flags = fcntl(fd.get(), F_GETFD);
    if (flags == -1) {
        const int error = errno;
        qCWarning(KWIN_CORE) << "SHM allocator fcntl(F_GETFD) failed"
                             << "size" << options.size
                             << "bytes" << bufferSize64
                             << "error" << strerror(error);
        return nullptr;
    }
    if (fcntl(fd.get(), F_SETFD, flags | FD_CLOEXEC) == -1) {
        const int error = errno;
        qCWarning(KWIN_CORE) << "SHM allocator fcntl(F_SETFD) failed"
                             << "size" << options.size
                             << "bytes" << bufferSize64
                             << "error" << strerror(error);
        return nullptr;
    }

    if (ftruncate(fd.get(), bufferSize) < 0) {
        const int error = errno;
        qCWarning(KWIN_CORE) << "SHM allocator ftruncate failed"
                             << "size" << options.size
                             << "bytes" << bufferSize64
                             << "error" << strerror(error);
        return nullptr;
    }
#endif

    ShmAttributes attributes{
        .fd = std::move(fd),
        .stride = stride,
        .offset = 0,
        .size = options.size,
        .format = options.format,
    };

    MemoryMap memoryMap(attributes.stride * attributes.size.height(), PROT_READ | PROT_WRITE, MAP_SHARED, attributes.fd.get(), attributes.offset);
    if (!memoryMap.isValid()) {
        const int error = errno;
        qCWarning(KWIN_CORE) << "SHM allocator mmap failed"
                             << "size" << options.size
                             << "stride" << stride
                             << "bytes" << bufferSize64
                             << "error" << strerror(error);
        return nullptr;
    }

    return new ShmGraphicsBuffer(std::move(attributes), std::move(memoryMap));
}

} // namespace KWin

#include "moc_shmgraphicsbufferallocator.cpp"
#include "shmgraphicsbufferallocator.moc"
