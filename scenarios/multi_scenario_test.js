// Multi-Scenario Test
// Tests running multiple scenarios in parallel

export const options = {
    scenarios: {
        fast: {
            workers: 2,
            duration: '3s',
            exec: 'fastPath',
        },
        slow: {
            workers: 1,
            duration: '5s',
            exec: 'slowPath',
        },
    },
};

export function fastPath() {
    print("[fast] Running fast iteration");
    sleep(0.1);
}

export function slowPath() {
    print("[slow] Running slow iteration");
    sleep(1);
}
