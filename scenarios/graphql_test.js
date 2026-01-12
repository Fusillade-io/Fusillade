// GraphQL Load Test Scenario
// Demonstrates using http.post for GraphQL queries and mutations

export const options = {
    workers: 2,
    duration: '15s',
};

// GraphQL endpoint (using public Countries API for testing)
const GRAPHQL_ENDPOINT = 'https://countries.trevorblades.com/graphql';

// GraphQL queries
const COUNTRY_QUERY = `
    query GetCountry($code: ID!) {
        country(code: $code) {
            name
            native
            capital
            emoji
            currency
            languages {
                code
                name
            }
        }
    }
`;

const CONTINENTS_QUERY = `
    query GetContinents {
        continents {
            code
            name
            countries {
                code
                name
            }
        }
    }
`;

// Helper function for GraphQL requests
function graphqlRequest(query, variables = {}, operationName = null) {
    const payload = JSON.stringify({
        query,
        variables,
        operationName
    });

    const headers = {
        'Content-Type': 'application/json',
    };

    return http.post(GRAPHQL_ENDPOINT, payload, {
        headers,
        name: operationName || 'GraphQL'
    });
}

export default function () {
    // Test 1: Query a specific country
    const countryRes = graphqlRequest(COUNTRY_QUERY, { code: 'US' }, 'GetCountry');

    check(countryRes, {
        'country query status 200': (r) => r.status === 200,
        'country has data': (r) => {
            const body = JSON.parse(r.body);
            return body.data && body.data.country && body.data.country.name === 'United States';
        },
    });

    sleep(0.1);

    // Test 2: Query all continents
    const continentsRes = graphqlRequest(CONTINENTS_QUERY, {}, 'GetContinents');

    check(continentsRes, {
        'continents query status 200': (r) => r.status === 200,
        'continents found': (r) => {
            const body = JSON.parse(r.body);
            return body.data && body.data.continents && body.data.continents.length > 0;
        },
    });

    sleep(0.1);

    // Test 3: Query random countries
    const countryCodes = ['DE', 'FR', 'JP', 'BR', 'AU', 'CA', 'GB'];
    const randomCode = countryCodes[Math.floor(Math.random() * countryCodes.length)];

    const randomCountryRes = graphqlRequest(COUNTRY_QUERY, { code: randomCode }, 'GetRandomCountry');

    check(randomCountryRes, {
        'random country status 200': (r) => r.status === 200,
        'random country has name': (r) => {
            const body = JSON.parse(r.body);
            return body.data && body.data.country && body.data.country.name;
        },
    });
}

// Lifecycle hooks
export function setup() {
    print('Starting GraphQL load test...');

    // Verify endpoint is reachable
    const testRes = graphqlRequest(COUNTRY_QUERY, { code: 'US' });
    if (testRes.status !== 200) {
        throw new Error('GraphQL endpoint not reachable');
    }

    return { startTime: Date.now() };
}

export function teardown(data) {
    const duration = Date.now() - data.startTime;
    print('GraphQL load test completed in ' + duration + 'ms');
}
