/* Independently authored Zebulon collection helpers. */
owned zebSortWith(values, direction, compare)
{
    local count = values.length();
    local owned first = new Vector(count);
    local owned second = new Vector(count);
    foreach (local value in values) { first.append(value); second.append(nil); }
    local source = first;
    local dest = second;
    for (local width = 1; width < count; )
    {
        for (local start = 0; start < count; )
        {
            local middle = start + (width < count - start ? width : count - start);
            local end = middle + (width < count - middle ? width : count - middle);
            local left = start;
            local right = middle;
            for (local out = start; out < end; ++out)
            {
                local takeLeft = nil;
                if (left < middle)
                {
                    if (right >= end) takeLeft = true;
                    else
                    {
                        local order = compare(source[left + 1], source[right + 1]);
                        takeLeft = (direction ? order >= 0 : order <= 0);
                    }
                }
                if (takeLeft) { dest[out + 1] = source[left + 1]; ++left; }
                else { dest[out + 1] = source[right + 1]; ++right; }
            }
            start = end;
        }
        local swap = source;
        source = dest;
        dest = swap;
        if (width > count / 2) break;
        width *= 2;
    }
    local owned result = source.toList();
    return move result;
}
