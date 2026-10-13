#pragma once

// Parsed before the capability check. Bound even an unfinished header line.
#define CPPHTTPLIB_REQUEST_URI_MAX_LENGTH 2048
#define CPPHTTPLIB_MAX_LINE_LENGTH 8192
#define CPPHTTPLIB_HEADER_MAX_LENGTH 8192
#define CPPHTTPLIB_HEADER_MAX_COUNT 64

// cpp-httplib 0.58: ThreadPool(base threads, maximum threads, queued requests).
// Zero for the third argument would permit an unbounded queue.
#define WRITER_HTTP_BASE_THREADS 8
#define WRITER_HTTP_MAX_THREADS 32
#define WRITER_HTTP_MAX_QUEUED_REQUESTS 32
#define WRITER_HTTP_READ_TIMEOUT_SECONDS 10
#define WRITER_HTTP_WRITE_TIMEOUT_SECONDS 30
#define WRITER_HTTP_KEEP_ALIVE_TIMEOUT_SECONDS 5
#define WRITER_HTTP_KEEP_ALIVE_MAX_COUNT 16
