// Independently authored fixture: `break label` leaves a labelled statement.
main()
{
    local n = 0;
guard:
    if (n == 0)
    {
        for (local i = 1; i <= 5; ++i)
        {
            n += i;
            if (i == 3)
                break guard;
        }
        n = 100;
    }
    "if: <<n>>.\n";

    local m = 0;
    local trace = 0;
group:
    {
        m = 1;
        switch (m)
        {
        case 1:
            m = 2;
            break group;
        default:
            m = 99;
            break;
        }
        m = 50;
    }
    "block: <<m>>.\n";

    local k = 0;
protect:
    {
        try
        {
            k = 1;
            break protect;
        }
        finally
        {
            trace = 7;
        }
    }
    "finally: <<k>>/<<trace>>.\n";

    local total = 0;
rows:
    for (local i = 1; i <= 3; ++i)
    {
        for (local j = 1; j <= 3; ++j)
        {
            if (j == 2)
                continue rows;
            total += 1;
        }
    }
    "loop: <<total>>.\n";
    return nil;
}
