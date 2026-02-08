// TypeScript import test scenario
// Validates that .ts module imports work

import { greet, add, multiply, createUser } from 'support/helpers.ts';

export const options = {
    workers: 1,
    duration: '1s',
};

export default function(): void {
    const greeting: string = greet('Fusillade');
    print(greeting);
    assertion(greeting, {
        'greeting contains name': (g: string) => g.includes('Fusillade'),
    });

    const sum: number = add(10, 20);
    assertion(sum, {
        'add works': (v: number) => v === 30,
    });

    const product: number = multiply(5, 6);
    assertion(product, {
        'multiply works': (v: number) => v === 30,
    });

    const user = createUser(1, 'Alice');
    assertion(user, {
        'user has id': (u: any) => u.id === 1,
        'user has name': (u: any) => u.name === 'Alice',
        'user has email': (u: any) => u.email === 'alice@example.com',
    });

    sleep(0.1);
}
