add_library(Fixture::dep ALIAS dep)
add_library(scope STATIC ${CHILD_SOURCE})
set(PROBE_SOURCE should_not_leak.c)
add_test(NAME value COMMAND probe [=[two words'$]=] 42 "")
