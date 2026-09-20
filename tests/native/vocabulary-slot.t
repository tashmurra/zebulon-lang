// Grammar slots select a part of speech from a labelled vocabulary relation.
// Matching binds entity identities rather than token text.
enum token tokWord;
property firstTokenIndex, lastTokenIndex, tokenList;
dictionary cmdDict;
property noun, adjective;
relation vocab(entity: Entity, word: Text, part: Vocabulary) many_to_many;

class Thing: object name = nil;
desk: Thing vocab = 'oak desk; large; table';
rug: Thing vocab = 'oak rug';

class Prod: object tag = nil item_ = nil;
grammar command(take): 'take' vocab.noun->item_ : Prod tag = 'take';
grammar command(rub): 'rub' vocab.adjective->item_ : Prod tag = 'rub';

startup()
{
    "> ";
    return nil;
}

turn(toks)
{
    local m = command.parseTokens(toks, cmdDict);
    if (m.length() == 0)
        "no match\n";
    else
    {
        "<<m[1].tag>> matched <<m[1].item_.length()>>:";
        foreach (local entity in m[1].item_)
            " <<entity.name>>";
        "\n";
    }
    "> ";
    return nil;
}
