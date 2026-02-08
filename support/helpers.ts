// TypeScript helper module for testing imports

export function greet(name: string): string {
    return `Hello, ${name}!`;
}

export function add(a: number, b: number): number {
    return a + b;
}

export function multiply(a: number, b: number): number {
    return a * b;
}

export interface User {
    id: number;
    name: string;
    email: string;
}

export function createUser(id: number, name: string): User {
    return {
        id,
        name,
        email: `${name.toLowerCase()}@example.com`,
    };
}
