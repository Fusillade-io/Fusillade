// Graceful Stop Test
// Tests that the engine waits for the current iteration to finish

export const options = {
    workers: 1,
    duration: '3s',
    stop: '3s',
};

export default function () {
    print("Starting iteration...");
    // Sleep for 2s. 
    // Iteration 1: 0s -> 2s (success)
    // Iteration 2: 2s -> 4s (starts before 3s limit, finishes after 3s)
    // Without graceful stop, Iteration 2 would stop at 3s.
    // With graceful stop, it should finish at 4s.
    sleep(2);
    print("Finished iteration");
}
