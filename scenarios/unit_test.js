export const options = {
    duration: '1s',
    workers: 1,
};

export default function() {
    describe("My Unit Tests", () => {
        test("should pass basic math", () => {
            expect(1 + 1).toBe(2);
        });

        test("should check object equality", () => {
            expect({a: 1}).toEqual({a: 1});
        });

        test("should handle failures gracefully", () => {
            // This is expected to fail and print an error but not crash the worker
            expect(true).toBe(false);
        });
    });
}
