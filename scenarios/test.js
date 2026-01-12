// Basic test scenario for test_basic_test_js test
export const options = {
    workers: 1,
    iterations: 1,
};

export default function () {
    print("Running test scenario");

    // Simple assertions
    check(true, {
        'true is truthy': (v) => v === true,
    });

    print("Test complete");
}
