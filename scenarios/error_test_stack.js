
function failDeep() {
    throw new Error("Deep error");
}

function callFail() {
    failDeep();
}

export default function() {
    print("Calling fail...");
    callFail();
}
