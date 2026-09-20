// Vocabulary rows carry a word, entity and part-of-speech property.
// Use property addresses such as &noun to distinguish the word roles.

enum token tokWord;
dictionary cmdDict;
property noun, adjective;
relation vocab(entity: Entity, word: Text, part: Vocabulary) many_to_many;

class Thing: object name = 'thing';
Thing template 'name';
desk: Thing 'desk';
rug: Thing 'rug';

// A grammar slot cannot name this relation yet: a slot has nowhere to say
// which part of speech it wants, so one is refused rather than compiled into a
// match that never fires. Tracked as .

show(label, words)
{
    "<<label>>: <<words.length()>>";
    foreach (local word in words)
        " <<word>>";
    "\n";
    return nil;
}

startup()
{
    // One entity, two parts of speech, and the same word in neither twice.
    vocab.set(desk, 'desk', &noun);
    vocab.set(desk, 'table', &noun);
    vocab.set(desk, 'oak', &adjective);
    vocab.set(rug, 'rug', &noun);
    show('desk nouns', vocab.all(desk, &noun));
    show('desk adjectives', vocab.all(desk, &adjective));
    // A word filed as a noun is not an adjective, which is the whole point.
    "'table' as noun: <<vocab.contains(desk, 'table', &noun) ? 'yes' : 'no'>>\n";
    "'table' as adjective: <<vocab.contains(desk, 'table', &adjective) ? 'yes' : 'no'>>\n";
    "> ";
    return nil;
}

turn(toks)
{
    // Vocabulary is data, so a turn can change what a thing answers to and ask
    // again, with separate part-of-speech labels.
    vocab.set(desk, 'bureau', &noun);
    show('desk nouns after adding', vocab.all(desk, &noun));
    "'bureau' as adjective: <<vocab.contains(desk, 'bureau', &adjective) ? 'yes' : 'no'>>\n";
    "> ";
    return nil;
}
