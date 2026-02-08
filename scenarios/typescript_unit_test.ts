// TypeScript unit test scenario
// Validates that describe/test/expect work with TS

export const options = {
    duration: '1s',
    workers: 1,
};

function add(a: number, b: number): number {
    return a + b;
}

function isEven(n: number): boolean {
    return n % 2 === 0;
}

interface Cart {
    items: number;
    total: number;
}

function calculateTotal(price: number, quantity: number): Cart {
    return { items: quantity, total: price * quantity };
}

export default function(): void {
    describe("TypeScript Unit Tests", () => {
        test("should add numbers", () => {
            expect(add(2, 3)).toBe(5);
            expect(add(-1, 1)).toBe(0);
        });

        test("should check even numbers", () => {
            expect(isEven(4)).toBe(true);
            expect(isEven(3)).toBe(false);
        });

        test("should calculate cart total", () => {
            const cart: Cart = calculateTotal(9.99, 3);
            expect(cart.items).toBe(3);
        });
    });
}
