// Ramping Test Scenario
// Tests the ramping-vus executor with multiple stages

export const options = {
    schedule: [
        { duration: '2s', target: 3 },  // Ramp up to 3 workers
        { duration: '2s', target: 3 },  // Stay at 3 workers
        { duration: '1s', target: 0 },  // Ramp down to 0
    ]
};

let iteration = 0;

export default function() {
    iteration++;
    print('Ramping iteration ' + iteration + ' on worker ' + __WORKER_ID);
    
    // Verify we're in a valid worker
    assertion(__WORKER_ID >= 0, {
        'worker ID is valid': (v) => v === true
    });
    
    sleep(0.2);
}

export function teardown() {
    print('Ramping test completed. Total iterations: ' + iteration);
}
