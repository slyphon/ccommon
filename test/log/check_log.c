#include <cc_log.h>

#ifdef HAVE_RUST
#include <rust/cc_log_rs.h>
#endif

#include <check.h>

#include <stdlib.h>
#include <stdio.h>
#include <cc_mm.h>

#define SUITE_NAME "log"
#define DEBUG_LOG  SUITE_NAME ".log"

static log_metrics_st metrics;

/*
 * utilities
 */
static void
test_setup(void)
{
    metrics = (log_metrics_st) { LOG_METRIC(METRIC_INIT) };
    log_setup(&metrics);
}

static void
test_teardown(void)
{
    log_teardown();
}

static void
test_reset(void)
{
    test_teardown();
    test_setup();
}

static char *
tmpname_create(void)
{
#define PATH "/tmp/temp.XXXXXX"
    char *path = malloc(sizeof(PATH) + 3);
    strcpy(path, PATH);
    mkdtemp(path);
    size_t len = strlen(path);
    path[len++] = '/';
    path[len++] = '1';
    path[len++] = 0;
    return path;
#undef PATH
}

static void
tmpname_destroy(char *path)
{
    unlink(path);
    path[strlen(path) - 2] = 0;
    rmdir(path);
    free(path);
}

static void
assert_file_contents(const char *tmpname, const char *str, size_t len)
{
    if (len == 0 && access(tmpname, F_OK) != -1) {
        return;
    }

    char *filedata = malloc(sizeof(char) * len);
    FILE *fp = fopen(tmpname, "r");
    ck_assert_ptr_ne(fp, NULL);

    ck_assert_uint_eq(len, fread(filedata, sizeof(char), len + 1, fp));
    ck_assert_int_eq(memcmp(filedata, str, len * sizeof(char)), 0);

    fclose(fp);
    free(filedata);
}

static void
_test_create_write_destroy(uint32_t buf_cap, bool reopen)
{
#define LOGSTR "foo bar baz"
    struct logger *logger = NULL;
    char *tmpname = tmpname_create();

    test_reset();

    logger = log_create(tmpname, buf_cap);
    ck_assert_ptr_ne(logger, NULL);

    if (reopen) {
        log_reopen(logger, NULL);
    }

    ck_assert_int_eq(log_write(logger, LOGSTR, sizeof(LOGSTR) - 1), 1);

    if (buf_cap == 0) {
        assert_file_contents(tmpname, LOGSTR, sizeof(LOGSTR) - 1);
    } else {
        assert_file_contents(tmpname, "", 0);
    }

    log_destroy(&logger);
    ck_assert_ptr_eq(logger, NULL);

    assert_file_contents(tmpname, LOGSTR, sizeof(LOGSTR) - 1);

    tmpname_destroy(tmpname);
#undef LOGSTR
}

START_TEST(test_create_write_destroy)
{
    _test_create_write_destroy(0, false);
}
END_TEST

START_TEST(test_create_large_buf_write_destroy)
{
    _test_create_write_destroy(100, false);
}
END_TEST

START_TEST(test_create_reopen_write_destroy)
{
    _test_create_write_destroy(0, true);
}
END_TEST

START_TEST(test_create_large_buf_reopen_write_destroy)
{
    _test_create_write_destroy(100, true);
}
END_TEST

static void
test_create_metrics(char *tmpname)
{
    struct logger *logger = NULL;
    test_reset();

    ck_assert_uint_eq(metrics.log_create.counter, 0);
    ck_assert_uint_eq(metrics.log_open.counter, 0);
    ck_assert_uint_eq(metrics.log_curr.counter, 0);

    logger = log_create(tmpname, 0);
    ck_assert_uint_eq(metrics.log_open.counter, tmpname == NULL ? 0 : 1);
    ck_assert_uint_eq(metrics.log_create.counter, 1);
    ck_assert_uint_eq(metrics.log_curr.counter, 1);
    ck_assert_uint_eq(metrics.log_destroy.counter, 0);

    log_destroy(&logger);

    ck_assert_uint_eq(metrics.log_destroy.counter, 1);
    ck_assert_uint_eq(metrics.log_curr.counter, 0);
}

START_TEST(test_create_metrics_file)
{
    char *tmpname = tmpname_create();
    test_create_metrics(tmpname);
    tmpname_destroy(tmpname);
}
END_TEST

START_TEST(test_create_metrics_stderr)
{
    test_create_metrics(NULL);
}
END_TEST

static void
test_write_metrics(char *tmpname, uint32_t buf_cap)
{
#define LOGSTR "foo"
    struct logger *logger;
    /**
     * number of writes before calling log_write
     * log_create write to the log, which makes the initial value not 0
     * this also makes it impossible to test log_write_bytes
     * maybe log_* functions should not use the same logging system and metrics?
     */
    size_t before;
    test_reset();

    logger = log_create(tmpname, buf_cap);
    before = metrics.log_write.counter;

    ck_assert_int_eq(log_write(logger, LOGSTR, sizeof(LOGSTR) - 1), 1);
    ck_assert_uint_eq(metrics.log_write.counter, before + 1);

    log_destroy(&logger);
#undef LOGSTR
}

START_TEST(test_write_metrics_file_buf)
{
    char *tmpname = tmpname_create();
    test_write_metrics(tmpname, 10);
    tmpname_destroy(tmpname);
}
END_TEST

START_TEST(test_write_metrics_stderr_buf)
{
    test_write_metrics(NULL, 10);
}
END_TEST

START_TEST(test_write_metrics_file_nobuf)
{
    char *tmpname = tmpname_create();
    test_write_metrics(tmpname, 0);
    tmpname_destroy(tmpname);
}
END_TEST

START_TEST(test_write_metrics_stderr_nobuf)
{
    test_write_metrics(NULL, 0);
}
END_TEST

START_TEST(test_write_skip_metrics)
{
#define LOGSTR "foo bar baz"
    struct logger *logger;
    test_reset();

    logger = log_create(NULL, 5);

    ck_assert_uint_eq(metrics.log_skip.counter, 0);
    ck_assert_uint_eq(metrics.log_skip_byte.counter, 0);

    ck_assert_int_eq(log_write(logger, LOGSTR, sizeof(LOGSTR) - 1), 0);
    ck_assert_uint_eq(metrics.log_skip.counter, 1);
    ck_assert_uint_eq(metrics.log_skip_byte.counter, sizeof(LOGSTR) - 1);

    log_destroy(&logger);
#undef LOGSTR
}
END_TEST


#ifdef HAVE_RUST

// NOTE: the returned pointer must have bstring_deinit called on it!
static struct bstring *
read_whole_file(char *tmpname) {
    FILE *fp = NULL;
    size_t fsize = 0;
    size_t rsize = 0;
    char *buffer = NULL;
    struct bstring *data = NULL;

    fp = fopen(tmpname , "rb");
    ck_assert_ptr_nonnull(fp);

    fseek(fp , 0L , SEEK_END);
    fsize = (size_t)ftell(fp);
    ck_assert_uint_lt(fsize, UINT32_MAX);
    ck_assert_uint_gt(fsize, 0);
    rewind(fp);

    buffer = malloc(fsize);
    ck_assert_ptr_nonnull(buffer);

    rsize = fread(buffer, fsize, 1, fp);
    ck_assert_int_eq(fclose(fp), 0);
    ck_assert_uint_eq(rsize, 1);

    data = malloc(sizeof(struct bstring));
    ck_assert_ptr_nonnull(data);

    data->len = (uint32_t)fsize;
    data->data = buffer;

    return data;
}

START_TEST(test_rust_logger_integration)
{
#define LOGSTR "this is the message to log"
    struct bstring msg = null_bstring;
    char *tmpname = NULL;
    struct logger *log = NULL;
    struct bstring *result = NULL;
    ck_assert(log_st_setup_rs() == LOG_STATUS_OK);

    tmpname = tmpname_create();
    log = log_create(tmpname, 0);

    ck_assert(log_st_set_rs(log, LOG_LEVEL_TRACE) == LOG_STATUS_OK);

    ck_assert_msg(log_st_is_setup_rs(), "log was not set up");

    bstring_set_raw(&msg, LOGSTR);
    ck_assert(log_st_log_rs(&msg, LOG_LEVEL_ERROR) == LOG_STATUS_OK);

    result = read_whole_file(tmpname);
    ck_assert_ptr_nonnull(result);
    ck_assert_uint_ge(result->len, msg.len);

    ck_assert_ptr_nonnull(
        memmem(result->data, result->len, msg.data, msg.len));

    bstring_deinit(result);
    cc_free(result);
    log_destroy(&log);
    tmpname_destroy(tmpname);
}
#undef LOGSTR
END_TEST

#endif /* HAVE_RUST */

/*
 * test suite
 */
static Suite *
log_suite(void)
{
    Suite *s = suite_create(SUITE_NAME);
    TCase *tc_log = tcase_create("log test");
    suite_add_tcase(s, tc_log);

    tcase_add_test(tc_log, test_create_write_destroy);
    tcase_add_test(tc_log, test_create_large_buf_write_destroy);
    tcase_add_test(tc_log, test_create_reopen_write_destroy);
    tcase_add_test(tc_log, test_create_large_buf_reopen_write_destroy);
    tcase_add_test(tc_log, test_create_metrics_file);
    tcase_add_test(tc_log, test_create_metrics_stderr);
    tcase_add_test(tc_log, test_write_metrics_file_buf);
    tcase_add_test(tc_log, test_write_metrics_stderr_buf);
    tcase_add_test(tc_log, test_write_metrics_file_nobuf);
    tcase_add_test(tc_log, test_write_metrics_stderr_nobuf);
    tcase_add_test(tc_log, test_write_skip_metrics);

#ifdef HAVE_RUST
    tcase_add_test(tc_log, test_rust_logger_integration);
#endif

    return s;
}
/**************
 * test cases *
 **************/

int
main(void)
{
    int nfail;

    /* setup */
    test_setup();

    Suite *suite = log_suite();
    SRunner *srunner = srunner_create(suite);
    srunner_set_log(srunner, DEBUG_LOG);
    srunner_run_all(srunner, CK_ENV); /* set CK_VEBOSITY in ENV to customize */
    nfail = srunner_ntests_failed(srunner);
    srunner_free(srunner);

    /* teardown */
    test_teardown();

    return (nfail == 0) ? EXIT_SUCCESS : EXIT_FAILURE;
}
