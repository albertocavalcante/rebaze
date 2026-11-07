package example.core;
import org.hamcrest.Matcher;
import org.hamcrest.CoreMatchers;
public class Core {
    public static Matcher<Integer> matcher() { return CoreMatchers.equalTo(42); }
}
