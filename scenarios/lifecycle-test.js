
export function setup() {
  print('EXEC_SETUP');
  return { val: 'passed_data' };
}

export default function(data) {
  print('EXEC_DEFAULT: ' + (data ? data.val : 'no_data'));
  sleep(0.01);
}

export function teardown(data) {
  print('EXEC_TEARDOWN: ' + (data ? data.val : 'no_data'));
}
