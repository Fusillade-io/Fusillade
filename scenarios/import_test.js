import { add, square } from 'support/math.js';

export default function() {
    let sum = add(10, 5);
    let sq = square(4);
    
    print('10 + 5 = ' + sum);
    print('4 squared = ' + sq);
    
    assertion(sum, { 'add works': (v) => v === 15 });
    assertion(sq, { 'square works': (v) => v === 16 });
}