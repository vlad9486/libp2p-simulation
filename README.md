# Libp2p simulation

Try using Model Checker with libp2p.

This crate replaces the `tcp` transport of the libp2p and uses libp2p-swarm. The type `Application` provides everything needed to wrap any libp2p application as an actor that can be used in the model checker (such as [`stateright`](https://github.com/stateright/stateright)).

Limitations:

* Assumes that the base libp2p transport is `TCP`, i.e. a transport that doesn't duplicate messages, maintains order, and ensures that everything is delivered.
* Simulates a trivial network topology. Any actor can listen to any address, and any address is public.

Known problems: 

* Some libp2p protocols are not deterministic: they use a random number generator, system time, and hashmap. Fortunately, `libp2p-swarm` and `libp2p-core` only use them in test code, or can be used in a way that preserves determinism.
* `libp2p-swarm` and `NetworkBehaviour` contain state. This makes it impossible to use `stateright`.
