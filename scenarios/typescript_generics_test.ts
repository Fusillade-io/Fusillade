// TypeScript generics and advanced types test
// Validates that complex TS features transpile correctly

export const options = {
    workers: 1,
    duration: '1s',
};

// Generics
function identity<T>(value: T): T {
    return value;
}

function firstElement<T>(arr: T[]): T | undefined {
    return arr[0];
}

// Type aliases and union types
type StringOrNumber = string | number;
type Result<T> = { ok: true; value: T } | { ok: false; error: string };

function parseNumber(input: StringOrNumber): Result<number> {
    if (typeof input === 'number') {
        return { ok: true, value: input };
    }
    const parsed = Number(input);
    if (isNaN(parsed)) {
        return { ok: false, error: `Cannot parse "${input}"` };
    }
    return { ok: true, value: parsed };
}

// Enum
enum Direction {
    Up = 'UP',
    Down = 'DOWN',
    Left = 'LEFT',
    Right = 'RIGHT',
}

function isVertical(dir: Direction): boolean {
    return dir === Direction.Up || dir === Direction.Down;
}

// Readonly and utility types
interface Config {
    readonly host: string;
    readonly port: number;
    timeout?: number;
}

function getUrl(config: Readonly<Config>): string {
    return `${config.host}:${config.port}`;
}

export default function(): void {
    // Test generics
    const num = identity<number>(42);
    assertion(num, { 'identity works': (v: number) => v === 42 });

    const first = firstElement<string>(['a', 'b', 'c']);
    assertion(first, { 'firstElement works': (v: string) => v === 'a' });

    // Test union types
    const result1 = parseNumber(42);
    assertion(result1, { 'number parsed': (r: any) => r.ok === true && r.value === 42 });

    const result2 = parseNumber('123');
    assertion(result2, { 'string parsed': (r: any) => r.ok === true && r.value === 123 });

    const result3 = parseNumber('abc');
    assertion(result3, { 'invalid returns error': (r: any) => r.ok === false });

    // Test enum
    assertion(Direction.Up, { 'enum value': (v: string) => v === 'UP' });

    // Test readonly config
    const config: Config = { host: 'localhost', port: 8080 };
    const url = getUrl(config);
    assertion(url, { 'url correct': (v: string) => v === 'localhost:8080' });

    sleep(0.1);
}
