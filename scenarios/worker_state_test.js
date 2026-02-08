// Worker state test scenario
// Tests __WORKER_STATE persistence across iterations and __WORKER_ID

export const options = {
    workers: 2,
    iterations: 3,
};

export default function() {
    // Track iteration count in worker state
    if (!__WORKER_STATE.count) {
        __WORKER_STATE.count = 0;
    }
    __WORKER_STATE.count++;

    print(`Worker ${__WORKER_ID}, iteration ${__ITERATION}, count: ${__WORKER_STATE.count}`);

    // Verify globals exist
    assertion(__WORKER_ID, {
        'worker ID is a number': (id) => typeof id === 'number',
        'worker ID is non-negative': (id) => id >= 0,
    });

    assertion(__ITERATION, {
        'iteration is a number': (it) => typeof it === 'number',
        'iteration is non-negative': (it) => it >= 0,
    });

    // Verify state persists
    assertion(__WORKER_STATE.count, {
        'count increments': (c) => c === __ITERATION + 1,
    });

    sleep(0.01);
}
