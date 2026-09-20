values: object
    input = 'C0A848D6'
    first = static toString(toInteger(input.substr(1, 2), 16), 10)
    rest = static input.substr(-2)
    phrase = static first + '.' + toString(toInteger(input.substr(3, 2), 16))
    another = static toString(192)
    sum = static 19 + 23
    negative = static toString(-1, 16)
    unicode = static 'aéz'.substr(2, 1)
;
aliases: object ;
main() {
    if (values.first != '192') return 'FAIL first';
    if ('192' != values.first) return 'FAIL reverse';
    if (values.first != values.another) return 'FAIL content';
    if (values.first == values.rest) return 'FAIL unequal';
    if (values.first == 192) return 'FAIL type';
    if (values.rest != 'D6') return 'FAIL suffix';
    if (values.phrase != '192.168') return 'FAIL concat';
    if (values.sum != 42) return 'FAIL numeric';
    if (values.negative != 'FFFFFFFF') return 'FAIL radix';
    if (values.unicode != 'é') return 'FAIL unicode';
    aliases.text = values.phrase;
    if (aliases.text != values.phrase) return 'FAIL borrow';
    return values.phrase;
}
