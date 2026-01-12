
export const options = {
    scenarios: {
        dataset_A: {
            executor: 'constant-vus',
            workers: 2,
            duration: '30s',
            exec: 'scenarioA',
        },
        dataset_B: {
            executor: 'constant-vus',
            workers: 2,
            duration: '30s',
            exec: 'scenarioB',
        },
    },
};

export function scenarioA() {
    console.log('[Scenario A] working...');
    fusillade.sleep('1s');
}

export function scenarioB() {
    console.log('[Scenario B] working...');
    fusillade.sleep('1s');
}
