#include <cc_mm.h>
#include <cc_pool.h>

#ifdef HAVE_RUST
#include <rust/cc_pool_rs.h>
#endif

#include <check.h>

#include <stdlib.h>
#include <stdio.h>

#define SUITE_NAME "pool"
#define DEBUG_LOG  SUITE_NAME ".log"


struct foo {
    STAILQ_ENTRY(foo) next;     /* next foo in pool */
    int d;
};

FREEPOOL(foo_pool, fooq, foo);
static struct foo_pool foop;

static struct foo *
foo_create(void)
{
    return (struct foo *)cc_alloc(sizeof(struct foo));
}

static void
foo_destroy(struct foo **foo)
{
    cc_free(*foo);
    *foo = NULL;
}

#ifdef HAVE_RUST

struct bar {
    int x;
    struct bstring bstring;
};

static void
bar_init(struct bar *b)
{
    b->x = 0;

    char *data = cc_alloc(4);
    ck_assert_ptr_nonnull(data);
    strncpy(data, "init", 4);

    b->bstring.data = data;
    b->bstring.len = 4;
}

static void
bar_reset(struct bar *b)
{
    b->x = 0;
    strncpy(b->bstring.data, "init", 4);
}

static void
bar_destroy(struct bar *b)
{
    bstring_deinit(&b->bstring);
}

#endif


/*
 * utilities
 */
static void
test_setup(void)
{
}

static void
test_teardown(void)
{
}

static void
test_reset(void)
{
    test_teardown();
    test_setup();
}

/*
 * test cases
 */
START_TEST(test_create_prealloc_destroy)
{
    struct foo *foo, *bar;
    uint32_t max = 10;

    test_reset();

    /* max is given, preallocate resources */
    FREEPOOL_CREATE(&foop, max);
    ck_assert_int_eq(foop.nmax, max);
    ck_assert(foop.initialized);

    FREEPOOL_PREALLOC(foo, &foop, max, next, foo_create);
    ck_assert_int_eq(foop.nfree, max);

    FREEPOOL_DESTROY(foo, bar, &foop, next, foo_destroy);
    ck_assert_int_eq(foop.nfree, 0);
    ck_assert(!foop.initialized);

    /* limit is set to 0, which means "unlimited" and no prealloc */
    FREEPOOL_CREATE(&foop, 0);
    ck_assert_int_eq(foop.nmax, UINT32_MAX);
    ck_assert(foop.initialized);

    FREEPOOL_PREALLOC(foo, &foop, 0, next, foo_create);
    ck_assert_int_eq(foop.nfree, 0);

    FREEPOOL_DESTROY(foo, bar, &foop, next, foo_destroy);
    ck_assert(!foop.initialized);
}
END_TEST

#ifdef HAVE_RUST

START_TEST(test_create_prealloc_destroy_rs)
{
    struct bar *a;
    uint32_t max = 2;
    struct bstring bs;
    bstring_init(&bs);
    bstring_set_raw(&bs, "init");

    struct pool_config_rs config = {
        .obj_size = sizeof(struct bar),
        .nmax = max,
        .init_callback = POOL_CALLBACK_RS(bar_init),
        .reset_callback = POOL_CALLBACK_RS(bar_reset),
        .destroy_callback = POOL_CALLBACK_RS(bar_destroy),
    };

    test_reset();

    struct pool_handle_rs *h = pool_create_handle_rs(&config);

    a = POOL_BORROW_RS(bar, h);

    ck_assert_int_eq(bstring_compare(&a->bstring, &bs), 0);
    POOL_RETURN_RS(h, a);
    ck_assert_ptr_null(a);
}
END_TEST

#endif

START_TEST(test_prealloc_borrow_return)
{
    struct foo *foo = NULL, *bar = NULL;
    uint32_t max = 10;

    test_reset();

    FREEPOOL_CREATE(&foop, max);
    FREEPOOL_PREALLOC(foo, &foop, max, next, foo_create);

    FREEPOOL_BORROW(foo, &foop, next, foo_create);
    ck_assert(foo != NULL);
    ck_assert_int_eq(foop.nused, 1);
    ck_assert_int_eq(foop.nfree, max - 1);
    FREEPOOL_BORROW(bar, &foop, next, foo_create);
    ck_assert(bar != NULL);
    ck_assert_int_eq(foop.nused, 2);
    ck_assert_int_eq(foop.nfree, max - 2);

    FREEPOOL_RETURN(foo, &foop, next);
    ck_assert_int_eq(foop.nused, 1);
    ck_assert_int_eq(foop.nfree, max - 1);
    FREEPOOL_RETURN(bar, &foop, next);
    ck_assert_int_eq(foop.nused, 0);
    ck_assert_int_eq(foop.nfree, max);

    FREEPOOL_DESTROY(foo, bar, &foop, next, foo_destroy);
}
END_TEST

START_TEST(test_noprealloc_borrow_return)
{
    struct foo *foo = NULL, *bar = NULL;

    test_reset();

    FREEPOOL_CREATE(&foop, 0);
    FREEPOOL_PREALLOC(foo, &foop, 0, next, foo_create);

    FREEPOOL_BORROW(foo, &foop, next, foo_create);
    ck_assert(foo != NULL);
    ck_assert_int_eq(foop.nused, 1);
    ck_assert_int_eq(foop.nfree, 0);
    FREEPOOL_BORROW(bar, &foop, next, foo_create);
    ck_assert(bar != NULL);
    ck_assert_int_eq(foop.nused, 2);
    ck_assert_int_eq(foop.nfree, 0);

    FREEPOOL_RETURN(foo, &foop, next);
    ck_assert_int_eq(foop.nused, 1);
    ck_assert_int_eq(foop.nfree, 1);
    FREEPOOL_RETURN(bar, &foop, next);
    ck_assert_int_eq(foop.nused, 0);
    ck_assert_int_eq(foop.nfree, 2);

    FREEPOOL_DESTROY(foo, bar, &foop, next, foo_destroy);
}
END_TEST


/*
 * test suite
 */
static Suite *
pool_suite(void)
{
    Suite *s = suite_create(SUITE_NAME);

    TCase *tc_pool = tcase_create("pool test");
    tcase_add_test(tc_pool, test_create_prealloc_destroy);
    tcase_add_test(tc_pool, test_prealloc_borrow_return);
    tcase_add_test(tc_pool, test_noprealloc_borrow_return);
    tcase_add_test(tc_pool, test_create_prealloc_destroy_rs);

    suite_add_tcase(s, tc_pool);

    return s;
}

int
main(void)
{
    int nfail;

    /* setup */
    test_setup();

    Suite *suite = pool_suite();
    SRunner *srunner = srunner_create(suite);
    srunner_set_log(srunner, DEBUG_LOG);
    srunner_run_all(srunner, CK_ENV); /* set CK_VEBOSITY in ENV to customize */
    nfail = srunner_ntests_failed(srunner);
    srunner_free(srunner);

    /* teardown */
    test_teardown();

    return (nfail == 0) ? EXIT_SUCCESS : EXIT_FAILURE;
}
