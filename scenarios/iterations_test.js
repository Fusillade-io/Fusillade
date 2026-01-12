// Per-VU Iterations Test
// Each worker runs exactly 3 iterations then exits

export const options = {
    workers: 2,
    iterations: 3,
};

export default function () {
    print("Worker " + __WORKER_ID + " iteration running");
    sleep(0.1);
}
