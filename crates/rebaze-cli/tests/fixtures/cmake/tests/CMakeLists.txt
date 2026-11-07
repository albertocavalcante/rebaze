add_library(Fixture::dep ALIAS dep)
add_test(NAME value COMMAND probe [=[two words'$]=] 42 "")
