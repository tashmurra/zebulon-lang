class Report: object construct(n) { number = n; } number = 0;
main() {
    local owned reports = new Vector(8);
    local owned first = new Report(7);
    local owned second = new Report(8);
    reports.append(first);
    reports.append(second);
    reports.append(first);
    reports.append(nil);
    reports.append(1);
    reports.append('message');
    local owned buffer = new StringBuffer();
    buffer.append('message');
    local owned text = toString(buffer);
    "Reports: <<reports.indexOf(first)>>/<<reports.indexOf(second)>>.\n";
    "Typed: <<reports.indexOf(nil)>>/<<reports.indexOf(1)>>/<<reports.indexOf(true) == nil ? 1 : 0>>.\n";
    "Text: <<reports.indexOf(text)>>/<<reports.indexOf('missing') == nil ? 1 : 0>>.\n";
    reports[1] = second;
    "Changed: <<reports.indexOf(first)>>/<<reports.indexOf(second)>>.\n";
    return nil;
}
